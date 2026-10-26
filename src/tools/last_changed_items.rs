//! `last_changed_items.php`. TODO: port. Until then, this answers 404.

use crate::web::AppError;

pub async fn handler() -> AppError {
    AppError::NotFound
}
