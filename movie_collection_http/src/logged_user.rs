use anyhow::Error;
pub use rust_auth_server::logged_user::{LoggedUser, AUTHORIZED_USERS};
use std::env::var;

use movie_collection_lib::pgpool::PgPool;

pub fn fill_from_db(pool: &PgPool) -> Result<(), Error> {
    let query = "SELECT email FROM authorized_users";
    let results: Result<Vec<_>, Error> = pool
        .get()?
        .query(query, &[])?
        .iter()
        .map(|row| {
            let email: String = row.try_get(0)?;
            Ok(LoggedUser { email })
        })
        .collect();
    let users = results?;

    if let Ok("true") = var("TESTENV").as_ref().map(|x| x.as_str()) {
        let user = LoggedUser {
            email: "user@test".to_string(),
        };
        AUTHORIZED_USERS.merge_users(&[user])?;
    }

    AUTHORIZED_USERS.merge_users(&users)
}
