use anyhow::{format_err, Error};
use deadpool_lapin::Config as LapinConfig;
use futures::stream::StreamExt;
use lapin::{
    options::BasicPublishOptions,
    options::{BasicAckOptions, BasicConsumeOptions},
    types::FieldTable,
    BasicProperties, Channel,
};
use serde::{Deserialize, Serialize};
use stack_string::StackString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::{
    fs::{self, File},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    task::spawn,
};

use crate::config::Config;
use crate::make_queue::make_queue_worker;
use crate::movie_collection::MovieCollection;
use crate::stdout_channel::StdoutChannel;
use crate::utils::parse_file_stem;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum JobType {
    Transcode,
    Move,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TranscodeServiceRequest {
    job_type: JobType,
    prefix: StackString,
    input_path: PathBuf,
    output_path: PathBuf,
}

impl TranscodeServiceRequest {
    pub fn create_transcode_request(config: &Config, path: &Path) -> Result<Self, Error> {
        let input_path = path.to_path_buf();
        let fstem = path.file_stem().ok_or_else(|| format_err!("No stem"))?;
        let script_file = config
            .home_dir
            .join("dvdrip")
            .join("jobs")
            .join(&fstem)
            .with_extension("sh");
        if Path::new(&script_file).exists() {
            Err(format_err!("File exists"))
        } else {
            let output_file = config
                .home_dir
                .join("dvdrip")
                .join("avi")
                .join(&fstem)
                .with_extension("mp4");
            let prefix = fstem.to_string_lossy().into_owned().into();

            Ok(Self {
                job_type: JobType::Transcode,
                prefix,
                input_path,
                output_path: output_file,
            })
        }
    }

    pub async fn create_remcom_request(
        config: &Config,
        path: &Path,
        directory: Option<&Path>,
        unwatched: bool,
    ) -> Result<Self, Error> {
        let ext = path
            .extension()
            .ok_or_else(|| format_err!("no extension"))?
            .to_string_lossy();

        if ext != "mp4" {
            Self::create_transcode_request(config, path)
        } else {
            let prefix = path.file_stem().unwrap().to_string_lossy().to_string();
            let output_dir = if let Some(d) = directory {
                let d = config
                    .preferred_dir
                    .join("Documents")
                    .join("movies")
                    .join(d);
                println!("{:?}", d);
                if !d.exists() {
                    return Err(format_err!(
                        "Directory {} does not exist",
                        d.to_string_lossy()
                    ));
                }
                d
            } else if unwatched {
                let d = config.preferred_dir.join("television").join("unwatched");
                if !d.exists() {
                    return Err(format_err!(
                        "Directory {} does not exist",
                        d.to_string_lossy()
                    ));
                }
                d
            } else {
                let file_stem = path.file_stem().unwrap().to_string_lossy();

                let (show, season, episode) = parse_file_stem(&file_stem);

                if season == -1 || episode == -1 {
                    panic!("Failed to parse show season {} episode {}", season, episode);
                }

                let d = config
                    .preferred_dir
                    .join("Documents")
                    .join("television")
                    .join(show.as_str())
                    .join(format!("season{}", season));
                if !d.exists() {
                    fs::create_dir_all(&d).await?;
                }
                d
            };

            let prefix = prefix.into();
            let input_path = path.to_path_buf();
            let output_path = output_dir.join(&format!("{}.mp4", prefix));

            Ok(Self {
                job_type: JobType::Move,
                prefix,
                input_path,
                output_path,
            })
        }
    }
}

pub struct TranscodeService {
    config: Config,
    queue: StackString,
}

impl TranscodeService {
    pub fn new(config: Config, queue: &str) -> Self {
        Self {
            config,
            queue: queue.into(),
        }
    }

    async fn open_transcode_channel() -> Result<Channel, Error> {
        let cfg = LapinConfig::default();
        let pool = cfg.create_pool();
        let conn = pool.get().await?;
        conn.create_channel().await.map_err(Into::into)
    }

    pub async fn publish_transcode_job(
        &self,
        payload: &TranscodeServiceRequest,
    ) -> Result<(), Error> {
        let chan = Self::open_transcode_channel().await?;
        let payload = serde_json::to_vec(&payload)?;
        chan.basic_publish(
            "",
            &self.queue,
            BasicPublishOptions::default(),
            payload,
            BasicProperties::default(),
        )
        .await?;
        Ok(())
    }

    pub async fn read_transcode_job(&self) -> Result<(), Error> {
        let chan = Self::open_transcode_channel().await?;
        let mut consumer = chan
            .basic_consume(
                &self.queue,
                &self.queue,
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await?;
        while let Some(delivery) = consumer.next().await {
            let (channel, delivery) = delivery?;
            let payload: TranscodeServiceRequest = serde_json::from_slice(&delivery.data)?;
            match payload.job_type {
                JobType::Transcode => {
                    self.run_transcode(&payload.prefix, &payload.input_path, &payload.output_path)
                        .await?
                }
                JobType::Move => {
                    self.run_move(&payload.prefix, &payload.input_path, &payload.output_path)
                        .await?
                }
            }
            channel
                .basic_ack(delivery.delivery_tag, BasicAckOptions::default())
                .await?;
        }
        Ok(())
    }

    async fn run_transcode(
        &self,
        prefix: &str,
        input_file: &Path,
        output_file: &Path,
    ) -> Result<(), Error> {
        if !input_file.exists() {
            return Ok(());
        }
        let output_path = output_file
            .file_name()
            .ok_or_else(|| format_err!("No Output File"))?;
        let output_path = self
            .config
            .home_dir
            .join("Documents")
            .join("movies")
            .join(output_path);
        let debug_output_path = self
            .config
            .home_dir
            .join("dvdrip")
            .join("log")
            .join(&format!("{}_mp4.out", prefix));
        let mut debug_output = File::create(&debug_output_path).await?;
        let mut p = Command::new("HandBrakeCLI")
            .args(&[
                "-i",
                input_file.to_string_lossy().as_ref(),
                "-o",
                output_file.to_string_lossy().as_ref(),
                "--preset",
                r#""Android 480p30""#,
            ])
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .spawn()?;
        if let Some(stdout) = p.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut buf = String::new();
            while let Ok(bytes) = reader.read_line(&mut buf).await {
                if bytes > 0 {
                    debug_output.write_all(buf.as_bytes()).await?;
                } else {
                    break;
                }
            }
        }
        if output_file.exists() {
            if fs::rename(&output_file, &output_path).await.is_err() {
                fs::copy(&output_file, &output_path).await?;
                fs::remove_file(&output_file).await?;
            }
        }
        if debug_output_path.exists() {
            let new_debug_output_path = self
                .config
                .home_dir
                .join("tmp_avi")
                .join(&format!("{}_mp4.out", prefix));
            fs::rename(&debug_output_path, &new_debug_output_path).await?;
        }
        Ok(())
    }

    async fn run_move(
        &self,
        show: &str,
        input_file: &Path,
        output_file: &Path,
    ) -> Result<(), Error> {
        if !input_file.exists() {
            return Ok(());
        }
        let show_path = self
            .config
            .home_dir
            .join("Documents")
            .join("movies")
            .join(&format!("{}.mp4", show));
        if !show_path.exists() {
            return Ok(());
        }
        let new_path = output_file.with_extension(".new");
        let task0 = spawn({
            let new_path = new_path.clone();
            async move { fs::copy(&show_path, &new_path).await }
        });
        if output_file.exists() {
            let old_path = output_file.with_extension(".old");
            fs::rename(&output_file, &old_path).await?;
        }
        task0.await??;
        fs::rename(&new_path, &output_file).await?;
        let stdout = StdoutChannel::new();
        make_queue_worker(&[], &[output_file.into()], false, &[], false, &stdout).await?;
        make_queue_worker(&[output_file.into()], &[], false, &[], false, &stdout).await?;
        let mc = MovieCollection::new();
        mc.make_collection().await?;
        mc.fix_collection_show_id().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Error;
    use std::{env::set_var, fs::create_dir_all, path::Path};

    use crate::{
        config::Config,
        transcode_service::{JobType, TranscodeServiceRequest},
    };

    fn init_env() {
        set_var(
            "PGURL",
            "postgresql://USER:PASSWORD@localhost:5432/movie_queue",
        );
        set_var("AUTHDB", "postgresql://USER:PASSWORD@localhost:5432/auth");
        set_var("MOVIE_DIRS", "/tmp");
        set_var("PREFERED_DISK", "/tmp");
        set_var("JWT_SECRET", "JWT_SECRET");
        set_var("SECRET_KEY", "SECRET_KEY");
        set_var("DOMAIN", "DOMAIN");
        set_var("SPARKPOST_API_KEY", "SPARKPOST_API_KEY");
        set_var("SENDING_EMAIL_ADDRESS", "SENDING_EMAIL_ADDRESS");
        set_var("CALLBACK_URL", "https://{DOMAIN}/auth/register.html");
        set_var("TRAKT_CLIENT_ID", "");
        set_var("TRAKT_CLIENT_SECRET", "");
    }

    #[tokio::test]
    async fn test_create_move_script() -> Result<(), Error> {
        init_env();
        let config = Config::new()?;
        let job_path = config.home_dir.join("dvdrip").join("jobs");
        create_dir_all(&job_path)?;
        let p = Path::new("mr_robot_s01_ep01.mp4");
        let payload =
            TranscodeServiceRequest::create_remcom_request(&config, p, None, false).await?;
        println!("{:?}", payload);
        assert_eq!(payload.job_type, JobType::Move);
        assert_eq!(&payload.input_path, p);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_move_script_movie() -> Result<(), Error> {
        init_env();
        let config = Config::new()?;
        let job_path = config.home_dir.join("dvdrip").join("jobs");
        create_dir_all(&job_path)?;
        let drama_dir = config
            .preferred_dir
            .join("Documents")
            .join("movies")
            .join("drama");
        create_dir_all(&drama_dir)?;
        let p = Path::new("a_night_to_remember.mp4");
        let payload = TranscodeServiceRequest::create_remcom_request(
            &config,
            p,
            Some(Path::new("drama")),
            false,
        )
        .await?;
        println!("{:?}", payload);
        assert_eq!(
            payload.output_path,
            config
                .preferred_dir
                .join("Documents/movies/drama/a_night_to_remember.mp4")
        );
        Ok(())
    }

    #[test]
    fn test_create_transcode_script() -> Result<(), Error> {
        init_env();
        let config = Config::new()?;
        let job_path = config.home_dir.join("dvdrip").join("jobs");
        create_dir_all(&job_path)?;
        let p = Path::new("mr_robot_s01_ep01.mkv");
        let payload = TranscodeServiceRequest::create_transcode_request(&config, p)?;
        println!("{:?}", payload);
        assert_eq!(&payload.input_path, p);
        let expected = config
            .home_dir
            .join("dvdrip")
            .join("avi")
            .join(&p.with_extension("mp4"));
        assert_eq!(payload.output_path, expected);
        Ok(())
    }
}
