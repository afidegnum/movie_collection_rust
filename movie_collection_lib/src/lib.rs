pub mod config;
pub mod imdb_episodes;
pub mod imdb_ratings;
pub mod imdb_utils;
pub mod make_list;
pub mod make_queue;
pub mod movie_collection;
pub mod movie_queue;
pub mod parse_imdb;
pub mod pgpool;
pub mod trakt_instance;
pub mod trakt_utils;
pub mod tv_show_source;
pub mod utils;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
