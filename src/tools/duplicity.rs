//! `duplicity.php and duplicity/api.php`. TODO: port. Until then, this answers 404.

use crate::web::AppError;

pub async fn redirect() -> AppError {
    AppError::NotFound
}

pub async fn api() -> AppError {
    AppError::NotFound
}
