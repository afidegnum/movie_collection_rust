use anyhow::{format_err, Error};
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::io;
use std::io::Write;
use std::path;

use crate::movie_collection::MovieCollection;
use crate::movie_queue::{MovieQueueDB, MovieQueueResult};
use crate::utils::{get_video_runtime, parse_file_stem};

pub fn make_queue_worker(
    add_files: Option<Vec<String>>,
    del_files: Option<Vec<String>>,
    do_time: bool,
    patterns: &[&str],
    do_shows: bool,
) -> Result<(), Error> {
    let mc = MovieCollection::new();
    let mq = MovieQueueDB::with_pool(&mc.pool);

    let stdout = io::stdout();

    if do_shows {
        let shows = mc.print_tv_shows()?;
        for show in shows {
            writeln!(stdout.lock(), "{}", show)?;
        }
    } else if let Some(files) = del_files {
        for file in files {
            if let Ok(idx) = file.parse::<i32>() {
                mq.remove_from_queue_by_idx(idx)?;
            } else {
                mq.remove_from_queue_by_path(&file)?;
            }
        }
    } else if let Some(files) = add_files {
        if files.len() == 1 {
            let max_idx = mq.get_max_queue_index()?;
            mq.insert_into_queue(max_idx + 1, &files[0])?;
        } else if files.len() == 2 {
            if let Ok(idx) = files[0].parse::<i32>() {
                writeln!(stdout.lock(), "inserting into {}", idx)?;
                mq.insert_into_queue(idx, &files[1])?;
            } else {
                for file in &files {
                    let max_idx = mq.get_max_queue_index()?;
                    mq.insert_into_queue(max_idx + 1, &file)?;
                }
            }
        } else {
            for file in &files {
                let max_idx = mq.get_max_queue_index()?;
                mq.insert_into_queue(max_idx + 1, &file)?;
            }
        }
    } else {
        let movie_queue = mq.print_movie_queue(&patterns)?;
        if do_time {
            let results: Result<Vec<_>, Error> = movie_queue
                .into_par_iter()
                .map(|result| {
                    let timeval = get_video_runtime(&result.path)?;
                    Ok((timeval, result))
                })
                .collect();

            for (timeval, result) in results? {
                writeln!(stdout.lock(), "{} {}", result, timeval)?;
            }
        } else {
            for result in movie_queue {
                writeln!(stdout.lock(), "{}", result)?;
            }
        }
    }
    Ok(())
}

pub fn movie_queue_http(queue: &[MovieQueueResult]) -> Result<Vec<String>, Error> {
    let mc = MovieCollection::new();

    let button = r#"<td><button type="submit" id="ID" onclick="delete_show('SHOW');"> remove </button></td>"#;

    queue
        .par_iter()
        .map(|row| {
            let path = path::Path::new(&row.path);
            let ext = path.extension()
                .ok_or_else(|| format_err!("Cannot determine extension"))?
                .to_string_lossy();
            let file_name = path
                .file_name()
                .ok_or_else(|| format_err!("Invalid path"))?
                .to_string_lossy().to_string();
            let file_stem = path
                .file_stem()
                .ok_or_else(|| format_err!("Invalid path"))?
                .to_string_lossy();
            let (_, season, episode) = parse_file_stem(&file_stem);

            let entry = if ext == "mp4" {
                let collection_idx = mc.get_collection_index(&row.path)?.unwrap_or(-1);
                format!(
                    r#"<a href="javascript:updateMainArticle('{}');">{}</a>"#,
                    &format!(
                        "{}/{}",
                        "/list/play", collection_idx
                    ), file_name
                )
            } else {
                file_name.to_string()
            };

            let entry = match row.link.as_ref() {
                Some(link) => {
                    format!("<tr>\n<td>{}</td>\n<td><a href={}>imdb</a></td>",
                        entry,
                        &format!(
                            "https://www.imdb.com/title/{}",
                            link
                        )
                    )
                },
                None => {
                    format!("<tr>\n<td>{}</td>\n",
                        entry
                    )
                },
            };
            let entry = format!(
                "{}\n{}",
                entry,
                button.replace("ID", &file_name).replace("SHOW", &file_name)
            );

            let entry = if ext == "mp4" {entry} else if season != -1 && episode != -1 {
                    format!(
                        r#"{}<td><button type="submit" id="{}" onclick="transcode('{}');"> transcode </button></td>"#,
                        entry, file_name, file_name)
                } else {
                    let entries: Vec<_> = row.path.split('/').collect();
                    let len_entries = entries.len();
                    let directory = entries[len_entries-2];
                    format!(
                        r#"{}<td><button type="submit" id="{}" onclick="transcode_directory('{}', '{}');"> transcode </button></td>"#,
                        entry, file_name, file_name, directory)
                };

            Ok(entry)
        })
        .collect()
}