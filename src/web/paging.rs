//! Links between the pages of a long list (`start`, `size` parameters);
//! shown by the `paging` macro of `templates/macros.html`.

pub struct Paging {
    /// `(link, "first–last")`
    pub previous: Option<(String, String)>,
    pub current: String,
    pub next: Option<(String, String)>,
    pub random: Option<String>,
    pub total: usize,
}

impl Paging {
    /// For a list of `total` entries shown from `start` on, `size` at a
    /// time; `query` is the rest of the page's query string (encoded).
    pub fn new(start: usize, size: usize, total: usize, query: &str) -> Self {
        let size = size.max(1);
        let link = |start: usize| match query {
            "" => format!("?start={start}&size={size}"),
            query => format!("?{query}&start={start}&size={size}"),
        };
        let range = |start: usize| format!("{}–{}", start + 1, (start + size).min(total));
        let last_start = total.saturating_sub(1) / size * size;
        Self {
            previous: (start > 0).then(|| start.saturating_sub(size)).map(|s| (link(s), range(s))),
            current: if total == 0 { "none".to_string() } else { range(start.min(last_start)) },
            next: (start + size < total).then(|| (link(start + size), range(start + size))),
            random: (total > size).then(|| link(rand::random_range(0..=last_start) / size * size)),
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let paging = Paging::new(100, 100, 250, "wiki=enwiki");
        assert_eq!(paging.previous, Some(("?wiki=enwiki&start=0&size=100".to_string(), "1–100".to_string())));
        assert_eq!(paging.current, "101–200");
        assert_eq!(paging.next, Some(("?wiki=enwiki&start=200&size=100".to_string(), "201–250".to_string())));
        assert!(paging.random.is_some());
        let single = Paging::new(0, 100, 5, "");
        assert_eq!((single.previous, single.next, single.random, single.current.as_str()), (None, None, None, "1–5"));
        assert_eq!(Paging::new(0, 100, 0, "").current, "none");
    }
}
