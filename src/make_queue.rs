extern crate clap;
extern crate failure;
extern crate movie_collection_rust;

use clap::{App, Arg};
use failure::Error;
use std::path::Path;

use movie_collection_rust::movie_collection::MovieCollection;
use movie_collection_rust::utils::get_version_number;

fn make_queue() -> Result<(), Error> {
    let matches = App::new("Parse IMDB")
        .version(get_version_number().as_str())
        .author("Daniel Boline <ddboline@gmail.com>")
        .about("Parse IMDB.com")
        .arg(
            Arg::with_name("add")
                .short("a")
                .long("add")
                .value_name("ADD")
                .takes_value(true)
                .multiple(true)
                .help("Add file(s)"),
        )
        .arg(
            Arg::with_name("remove")
                .short("r")
                .long("remove")
                .value_name("REMOVE")
                .takes_value(true)
                .multiple(true)
                .help("Remove entry by index OR filename"),
        )
        .arg(
            Arg::with_name("time")
                .short("t")
                .long("time")
                .takes_value(false)
                .help("Get runtime of file"),
        )
        .arg(
            Arg::with_name("patterns")
                .value_name("PATTERNS")
                .help("Patterns"),
        )
        .get_matches();
    let add_files: Option<Vec<_>> = matches
        .values_of("add")
        .map(|v| v.map(|s| s.to_string()).collect());
    let del_files: Option<Vec<_>> = matches
        .values_of("remove")
        .map(|v| v.map(|s| s.to_string()).collect());
    let do_time = matches.is_present("time");
    let patterns: Option<Vec<_>> = matches
        .values_of("patterns")
        .map(|v| v.map(|s| s.to_string()).collect());

    println!("{:?} {:?} {:?}", add_files, del_files, patterns);

    let mq = MovieCollection::new();

    if let Some(files) = del_files {
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
                println!("inserting into {}", idx);
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
        let results = mq.print_movie_queue(&patterns.unwrap_or_else(|| Vec::new()))?;
        for result in results {
            println!("{}", result);
        }
    }
    Ok(())
}

fn main() {
    make_queue().unwrap();
}