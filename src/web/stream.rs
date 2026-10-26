//! Large text answers made from database rows, sent while the rows arrive
//! instead of being built in memory first.

use super::AppError;
use axum::body::{Body, Bytes};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use wikimisc::mysql_async::prelude::{FromRow, Queryable};
use wikimisc::mysql_async::{Conn, Params};

/// Bytes collected before they are sent.
const FLUSH_AT: usize = 64 * 1024;
/// A client that reads nothing for this long is given up on, so that it
/// does not hold a database connection.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

type Sender = mpsc::Sender<Result<Bytes, std::io::Error>>;

/// Sends a chunk; false if the client is gone or stalled.
async fn send(sender: &Sender, chunk: Result<Bytes, std::io::Error>) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, sender.send(chunk)).await, Ok(Ok(())))
}

/// A body of `head`, then each row of the queries (`(sql, params)`, run one
/// after the other) as `format` makes it (`None` skips the row; the flag says
/// whether nothing came before), then `tail`. Returns once the first query
/// has started, so that its failure is an error, not a cut-off body; a later
/// failure breaks off the body.
pub async fn query_body<T, F>(
    mut conn: Conn,
    queries: Vec<(String, Params)>,
    head: String,
    tail: String,
    mut format: F,
) -> Result<Body, AppError>
where
    T: FromRow + Send + Unpin + 'static,
    F: FnMut(T, bool) -> Option<String> + Send + 'static,
{
    let (started, has_started) = oneshot::channel::<anyhow::Result<()>>();
    let (sender, receiver) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tokio::spawn(async move {
        let mut started = Some(started);
        let mut buffer = head;
        let mut first = true;
        for (sql, params) in queries {
            let mut rows = match conn.exec_stream::<T, _, _>(sql, params).await {
                Ok(rows) => rows,
                Err(e) => {
                    match started.take() {
                        Some(started) => drop(started.send(Err(e.into()))),
                        None => fail(&sender, e).await,
                    }
                    return;
                }
            };
            if let Some(started) = started.take() {
                let _ = started.send(Ok(()));
            }
            while let Some(row) = rows.next().await {
                let row = match row {
                    Ok(row) => row,
                    Err(e) => return fail(&sender, e).await,
                };
                if let Some(text) = format(row, first) {
                    buffer.push_str(&text);
                    first = false;
                }
                // Stops when the client is gone.
                if buffer.len() >= FLUSH_AT && !send(&sender, Ok(Bytes::from(std::mem::take(&mut buffer)))).await {
                    return;
                }
            }
        }
        if let Some(started) = started.take() {
            let _ = started.send(Ok(()));
        }
        buffer.push_str(&tail);
        send(&sender, Ok(Bytes::from(buffer))).await;
    });
    has_started.await.map_err(|_| AppError::Internal("query task ended".to_string()))??;
    let chunks = futures::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    Ok(Body::from_stream(chunks))
}

/// Breaks off the body.
async fn fail(sender: &Sender, e: wikimisc::mysql_async::Error) {
    tracing::warn!("streamed query failed: {e}");
    send(sender, Err(std::io::Error::other("query failed"))).await;
}
