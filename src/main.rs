use crate::api::{File, FilesXml, Source};
use anyhow::{anyhow, Context};
use clap::Parser;
use futures::StreamExt;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use reqwest::{redirect::Policy, Client, Response};
use std::{path::Path, str::FromStr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt}, task::JoinSet
};

mod api;

const BASE: &str = "https://archive.org";

#[derive(Parser, Debug)]
struct Args {
    /// Resource name to download files from IA
    resource_name: String,
    /// Skip duplicate files based on file signatures
    #[clap(long = "sd", default_value_t = false)]
    skip_duplicates: bool,
    /// Remove any files marked as derivatives by IA
    #[clap(long = "rd", default_value_t = false)]
    remove_derivatives: bool,
    /// Number of threads to use for downloading
    #[clap(short = 't', default_value_t = 2)]
    threads: usize,
    /// Directory to save downloads to
    /// By default downloads to a directory of the resource name
    #[clap(short = 'd', default_value = "auto")]
    download_directory: String,
    /// Number of retries to attempt on failed downloads
    #[clap(short = 'r', default_value_t = 3)]
    retries: usize,
    /// Regex to mach against file names to determine what to download
    #[clap(short = 'f')]
    filter: Option<String>
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();

    if args.download_directory == "auto" {
        args.download_directory.clone_from(&args.resource_name);
    }

    let _ = tokio::fs::create_dir_all(&args.download_directory).await;

    let spinner = ProgressBar::new_spinner()
        .with_message(format!("Fetching files for {}", args.resource_name));
    spinner.enable_steady_tick(Duration::from_millis(100));

    let mut files = fetch_files(&args).await?.files;

    spinner.finish_and_clear();

    // files.sort_by_key(|file| file.epoch_creation_date);

    if args.skip_duplicates {
        let before = files.len();
        files.dedup_by(|a, b| a.crc32.is_some() && a.crc32 == b.crc32);
        files.dedup_by(|a, b| a.md5.is_some() && a.md5 == b.md5);
        files.dedup_by(|a, b| a.sha1.is_some() && a.sha1 == b.sha1);

        let diff = before - files.len();
        if diff != 0 {
            println!("Filtered out {diff} duplicates");
        }
    }

    if args.remove_derivatives {
        let before = files.len();
        files.retain(|f| f.source != Source::Derivative);

        let diff = before - files.len();
        if diff != 0 {
            println!("Filtered out {diff} derivatives");
        }
    }

    if let Some(filter) = &args.filter {
        let regex =
            regex_lite::Regex::from_str(filter).context("failed to parse your regex filter")?;

        let before = files.len();
        files.retain(|f| regex.is_match(&f.name));

        let diff = before - files.len();
        if diff != 0 {
            println!("Filtered out {diff} files based on filter");
        }
    }

    let total_size = files.iter().map(|f| f.size.unwrap_or(0)).sum();

    println!("Downloading {} files with a total size of {}", files.len(), HumanBytes(total_size));

    let mpb = MultiProgress::new();
    let sty = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] {msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
        .progress_chars("#>-");

    let global_sty = ProgressStyle::default_bar()
        .template("{spinner.blue} [{elapsed_precise}] Files [{bar:40.cyan/blue}] {pos}/{len}")?
        .progress_chars("#>-");

    let global_pb =
        ProgressBar::new(files.len() as u64).with_style(global_sty).with_message("Files");

    let mut task_group = JoinSet::new();

    // Remainder will be accounted for by last thread
    let chunk_size = files.len() / args.threads;

    // SAFETY: `args` will NEVER be dropped before the entire program ends, after all threads have exited
    let args_r: &'static Args = unsafe { std::mem::transmute(&args) };

    for i in 0..args.threads {
        let mut chunk = files.drain(..chunk_size).collect::<Vec<_>>();

        if i == args.threads - 1 {
            chunk.append(&mut files);
        }

        let local_global_pb = ProgressBar::clone(&global_pb);
        let pb = mpb.add(ProgressBar::no_length().with_style(sty.clone()));
        task_group.spawn(async move {
            let client = Client::builder()
                .pool_idle_timeout(None)
                .redirect(Policy::limited(15))
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.3")
                .build()
                .expect("failed to build client");

            for file in chunk {
                pb.reset();

                let t = &file.name;
                pb.set_message(t.trim().to_owned());

                match download_file(args_r, &file, &pb, &client)
                    .await
                {
                    Err(e) => pb.println(format!("Failed to download {t}: {e:?}")),
                    Ok(()) => pb.println(format!("Downloaded {t}"))
                }

                pb.tick();

                local_global_pb.inc(1);
                local_global_pb.tick();
            }

            pb.finish_and_clear();
        });
    }

    // Make sure is last pb
    let global_pb = mpb.add(global_pb);
    global_pb.tick();

    while let Some(t) = task_group.join_next().await {
        if let Err(why) = t {
            println!("Thread failed: {why:?}",);
        }
    }

    global_pb.finish_with_message("All downloads complete");
    global_pb.tick();

    Ok(())
}

async fn fetch_files(args: &Args) -> anyhow::Result<FilesXml> {
    let url = format!("{BASE}/download/{}/{}_files.xml", args.resource_name, args.resource_name);
    let text = reqwest::get(&url).await?.text().await?;
    quick_xml::de::from_str(&text).map_err(Into::into)
}

#[derive(Debug)]
enum FileCheck {
    HashMismatch,
    SizeMismatch,
    BadMetadata,
    Ok
}

async fn check_hash(open_file: &mut tokio::fs::File, meta: &File) -> anyhow::Result<FileCheck> {
    let Some(server_crc32) = &meta.crc32 else { return Ok(FileCheck::BadMetadata) };

    let mut hasher = crc32fast::Hasher::new();

    let mut size = 0;
    let mut buf = [0; 4096];

    loop {
        let b = open_file.read(&mut buf).await?;
        size += b;

        if b == 0 {
            break;
        }

        hasher.update(&buf[..b]);
    }

    let local_crc32 = format!("{:x}", hasher.finalize());
    if server_crc32 != &local_crc32 {
        return Ok(FileCheck::HashMismatch);
    }

    if let Some(server_size) = meta.size {
        if server_size != size as u64 {
            return Ok(FileCheck::SizeMismatch);
        }
    }

    Ok(FileCheck::Ok)
}

async fn download_file(
    args: &Args,
    file: &File,
    pb: &ProgressBar,
    client: &Client
) -> anyhow::Result<()> {
    let path = Path::new(&args.download_directory).join(&file.name);
    if path.exists() {
        if let Ok(mut open_file) = tokio::fs::File::open(&path).await {
            match check_hash(&mut open_file, file).await {
                Ok(FileCheck::Ok) => {
                    pb.println(format!("Local hash & size match => skipping {}", file.name));
                    return Ok(());
                }
                Err(why) => pb.println(format!(
                    "Failed to check local hash for {} {why:?}, redownloading",
                    file.name
                )),
                Ok(why) => pb.println(format!("{why:?} mismatch for {}, redownloading", file.name))
            }
        }
    }

    if let Some(p) = path.parent() {
        let _ = tokio::fs::create_dir_all(p).await;
    }

    let url = format!("{BASE}/download/{}/{}", args.resource_name, file.name);
    let mut response: Result<Response, anyhow::Error> = Err(anyhow!("UNREACHABLE"));
    for i in 0..args.retries {
        response = client.get(&url).send().await.map_err(Into::into);

        let Err(why) = &response else {
            break;
        };

        pb.println(format!("Failed to download {}: {why:?}", file.name));
        if i == args.retries - 1 {
            return Err(unsafe { response.unwrap_err_unchecked() });
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let response = unsafe { response.unwrap_unchecked() };

    if let Some(content_length) = response.content_length() {
        pb.set_length(content_length);
    }

    let mut file = tokio::fs::File::create(&path).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;

        pb.tick();

        tokio::io::copy(&mut &chunk[..], &mut file).await?;

        pb.inc(chunk.len() as u64);
    }

    file.flush().await?;

    Ok(())
}
