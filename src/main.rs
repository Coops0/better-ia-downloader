use crate::api::{FileMeta, FilesXml, Source};
use anyhow::{anyhow, Context};
use clap::Parser;
use color_print::{cformat, cprintln};
use futures::StreamExt;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use reqwest::{redirect::Policy, Client, Response};
use std::{path::Path, str::FromStr, sync::Arc, time::Duration};
use tokio::{
    fs::File, io::{AsyncReadExt, AsyncWriteExt, BufWriter}, task::JoinSet
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
    filter: Option<String>,
    /// Skip validation of file hashes and sizes to prevent redownloading already downloaded files
    #[clap(long = "no-check", default_value_t = false)]
    no_check: bool,
    /// Print extra information
    #[clap(long = "verbose", short = 'v', default_value_t = false)]
    verbose: bool
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();

    if args.download_directory == "auto" {
        args.download_directory.clone_from(&args.resource_name);
    }

    let _ = tokio::fs::create_dir_all(&args.download_directory).await;

    let spinner = ProgressBar::new_spinner()
        .with_message(cformat!("<blue>Fetching files for {}</>", args.resource_name));
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
            cprintln!("<cyan>Filtered out {diff} duplicates</>");
        }
    }

    if args.remove_derivatives {
        let before = files.len();
        files.retain(|f| f.source != Source::Derivative);

        let diff = before - files.len();
        if diff != 0 {
            cprintln!("<cyan>Filtered out {diff} derivatives</>");
        }
    }

    if let Some(filter) = &args.filter {
        let regex =
            regex_lite::Regex::from_str(filter).context("failed to parse your regex filter")?;

        let before = files.len();
        files.retain(|f| regex.is_match(&f.name));

        let diff = before - files.len();
        if diff != 0 {
            cprintln!("<cyan>Filtered out {diff} files based on filter</cyan>");
        }
    }

    let total_size = files.iter().map(|f| f.size.unwrap_or(0)).sum();

    cprintln!(
        "<blue>Downloading {} files with a total size of {}</>",
        files.len(),
        HumanBytes(total_size)
    );

    let mpb = MultiProgress::new();
    let sty = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] {msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
        .progress_chars("#>-");

    let global_sty = ProgressStyle::default_bar()
        .template("{spinner.blue} [{elapsed_precise}] Files [{bar:80.cyan/blue}] {pos}/{len}")?;
    let global_pb =
        ProgressBar::new(files.len() as u64).with_style(global_sty).with_message("Files");

    let mut task_group = JoinSet::new();

    let client = Arc::new(Client::builder()
        .pool_idle_timeout(None)
        .tcp_nodelay(true)
        .pool_idle_timeout(None)
        .redirect(Policy::limited(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.3")
        .build()
        .expect("failed to build client"));

    let mut chunked_workloads: Vec<Vec<FileMeta>> =
        std::iter::repeat_with(Vec::new).take(args.threads).collect();

    for (i, file) in files.into_iter().enumerate() {
        chunked_workloads[i % args.threads].push(file);
    }

    // SAFETY: `args` will NEVER be dropped before the entire program ends, after all threads have exited
    let args_r: &'static Args = unsafe { std::mem::transmute(&args) };

    for chunk in chunked_workloads {
        let local_global_pb = ProgressBar::clone(&global_pb);
        let pb = mpb.add(ProgressBar::no_length().with_style(sty.clone()));
        let client = Arc::clone(&client);

        task_group.spawn(async move {
            for file in chunk {
                pb.reset();

                let t = &file.name.trim().to_owned();
                pb.set_message(t.clone());

                match download_file(args_r, &file, &pb, &client).await {
                    Err(e) => pb.println(cformat!("<red>Failed to download {t}: {e:?}</>")),
                    Ok(()) => {
                        if args.verbose {
                            pb.println(cformat!("<bright-magenta>Downloaded {t}</>"));
                        }
                    }
                }

                pb.tick();

                local_global_pb.inc(1);
            }

            pb.finish_and_clear();
        });
    }

    // Make sure is last pb
    let global_pb = mpb.add(global_pb);
    global_pb.tick();
    global_pb.enable_steady_tick(Duration::from_millis(500));

    while let Some(t) = task_group.join_next().await {
        if let Err(why) = t {
            cprintln!("<red>Thread failed: {why:?}</>");
        }
    }

    global_pb.finish_with_message("All downloads complete");

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

async fn check_hash(open_file: &mut File, meta: &FileMeta) -> anyhow::Result<FileCheck> {
    let Some(server_crc32) = &meta.crc32 else { return Ok(FileCheck::BadMetadata) };

    let mut hasher = crc32fast::Hasher::new();

    let mut size = 0;
    let mut buf = vec![0; 64 * 1024];

    loop {
        let b = open_file.read(&mut buf).await?;
        if b == 0 {
            break;
        }

        size += b;
        hasher.update(&buf[..b]);
    }

    let local_crc32 = format!("{:x}", hasher.finalize());
    drop(buf);

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
    meta: &FileMeta,
    pb: &ProgressBar,
    client: &Client
) -> anyhow::Result<()> {
    let path = Path::new(&args.download_directory).join(&meta.name);
    if !args.no_check && path.exists() {
        if let Ok(mut open_file) = File::open(&path).await {
            let message = match check_hash(&mut open_file, meta).await {
                Ok(FileCheck::Ok) => {
                    if args.verbose {
                        pb.println(cformat!(
                            "<green>Local hash & size match => skipping {}</>",
                            meta.name
                        ));
                    }
                    return Ok(());
                }
                Err(why) => cformat!(
                    "<yellow>Failed to check local hash for {} {why:?}, redownloading</>",
                    meta.name
                ),
                Ok(why) => cformat!("<yellow>{why:?} mismatch for {}, redownloading</>", meta.name)
            };

            if args.verbose {
                pb.println(message);
            }
        }
    }

    if let Some(p) = path.parent() {
        let _ = tokio::fs::create_dir_all(p).await;
    }

    let url = format!("{BASE}/download/{}/{}", args.resource_name, meta.name);
    let mut response: Result<Response, anyhow::Error> = Err(anyhow!("UNREACHABLE"));
    for i in 0..args.retries {
        response = client.get(&url).send().await.map_err(Into::into);

        let Err(why) = &response else { break };

        pb.println(cformat!("<red>Failed to download {}: {why:?}</>", meta.name));
        if i == args.retries - 1 {
            return Err(unsafe { response.unwrap_err_unchecked() });
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let response = unsafe { response.unwrap_unchecked() };

    if let Some(content_length) = response.content_length() {
        pb.set_length(content_length);
    }

    let file =
        File::options().write(true).read(false).create(true).truncate(true).open(&path).await?;

    let mut writer = BufWriter::new(file);

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;

        pb.tick();

        writer.write_all(&chunk).await?;

        pb.inc(chunk.len() as u64);
    }

    writer.flush().await?;

    Ok(())
}
