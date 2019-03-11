#![allow(clippy::needless_pass_by_value)]

use subprocess::Exec;

use movie_collection_rust::common::config::Config;
use movie_collection_rust::http::movie_queue_app::start_app;

fn main() {
    env_logger::init();

    let config = Config::with_config();
    let command = "rm -f /var/www/html/videos/partial/*";
    Exec::shell(command).join().unwrap();

    let sys = actix::System::new("movie_queue");
    start_app(config);
    let _ = sys.run();
}
