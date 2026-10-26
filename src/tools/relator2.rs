//! `relator2/api.php`. TODO: port. Until then, this answers 404.

use crate::web::AppError;

pub async fn api() -> AppError {
    AppError::NotFound
}
