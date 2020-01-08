use actix_threadpool::BlockingError;
use actix_web::{error::ResponseError, HttpResponse};
use anyhow::Error as AnyhowError;
use std::fmt::Debug;
use subprocess::PopenError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Internal Server Error")]
    InternalServerError,
    #[error("BadRequest: {0}")]
    BadRequest(String),
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Anyhow error {0}")]
    AnyhowError(#[from] AnyhowError),
    #[error("blocking error {0}")]
    BlockingError(String),
    #[error("Popen error {0}")]
    PopenError(#[from] PopenError),
}

// impl ResponseError trait allows to convert our errors into http responses with appropriate data
impl ResponseError for ServiceError {
    fn error_response(&self) -> HttpResponse {
        match *self {
            Self::BadRequest(ref message) => HttpResponse::BadRequest().json(message),
            Self::Unauthorized => HttpResponse::Ok()
                .content_type("text/html; charset=utf-8")
                .body(
                    include_str!("../../templates/login.html")
                        .replace("main.css", "/auth/main.css")
                        .replace("main.js", "/auth/main.js"),
                ),
            _ => {
                HttpResponse::InternalServerError().json("Internal Server Error, Please try later")
            }
        }
    }
}

impl<T: Debug> From<BlockingError<T>> for ServiceError {
    fn from(item: BlockingError<T>) -> Self {
        Self::BlockingError(format!("{:?}", item))
    }
}
