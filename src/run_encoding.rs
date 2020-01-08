use crossbeam_utils::thread;

use movie_collection_lib::config::Config;
use movie_collection_lib::utils::read_transcode_jobs_from_queue;

fn main() {
    env_logger::init();
    let config = Config::with_config().unwrap();

    thread::scope(|s| {
        let a = s.spawn(|_| read_transcode_jobs_from_queue(&config.transcode_queue));
        let b = s.spawn(|_| read_transcode_jobs_from_queue(&config.remcom_queue));
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    })
    .unwrap();
}
