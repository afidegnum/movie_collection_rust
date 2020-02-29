use anyhow::Error;
use log::debug;
pub use rust_auth_server::logged_user::{LoggedUser, AUTHORIZED_USERS, TRIGGER_DB_UPDATE};
use std::env::var;

use movie_collection_lib::pgpool::PgPool;

pub async fn fill_from_db(pool: &PgPool) -> Result<(), Error> {
    debug!("{:?}", *TRIGGER_DB_UPDATE);
    let users = if TRIGGER_DB_UPDATE.check() {
        let query = "SELECT email FROM authorized_users";
        let results: Result<Vec<_>, Error> = pool
            .get()
            .await?
            .query(query, &[])
            .await?
            .iter()
            .map(|row| {
                let email: String = row.try_get(0)?;
                Ok(LoggedUser { email })
            })
            .collect();
        results?
    } else {
        AUTHORIZED_USERS.get_users()
    };
    if let Ok("true") = var("TESTENV").as_ref().map(String::as_str) {
        let user = LoggedUser {
            email: "user@test".to_string(),
        };
        AUTHORIZED_USERS.merge_users(&[user])?;
    }
    AUTHORIZED_USERS.merge_users(&users)?;

    debug!("{:?}", *AUTHORIZED_USERS);
    Ok(())
}
