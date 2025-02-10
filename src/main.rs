use std::{collections::HashMap, path::PathBuf};

use clap::Parser;
use futures_concurrency::future::Join;
use reqwest::Client;
use scraper::{Html, Selector};
use tokio::sync::Mutex;
use tracing::error;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Collection root folder
    folder: PathBuf,

    /// Number of results
    #[clap(short, long, default_value = "10")]
    num_results: usize,

    /// Show additional data, like average distance
    #[clap(short, long, default_value = "false")]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let client = Client::new();

    let selector = Selector::parse("div#gnodMap a.S").expect("wrong selector");

    let new_artists = Mutex::new(HashMap::new());
    let mut join_set = Vec::new();
    let mut folders = tokio::fs::read_dir(args.folder).await?;
    let mut existing_artists = Vec::new();
    while let Ok(Some(entry)) = folders.next_entry().await {
        if !entry.metadata().await?.is_dir() {
            continue;
        }

        let artist = entry
            .file_name()
            .into_string()
            .map_err(|s| anyhow::anyhow!("Invalid folder name {s:?}"))?;
        let url = format!("https://www.music-map.com/{artist}");
        existing_artists.push(artist);

        join_set.push({
            let new_artists = &new_artists;
            let client = &client;
            let selector = &selector;
            async move {
                let page = Html::parse_document(
                    &client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|err| error!("Error calling {url}: {err}"))?
                        .error_for_status()
                        .map_err(|err| error!("Received error from {url}: {err}"))?
                        .text()
                        .await
                        .map_err(|err| error!("Error reading {url}: {err}"))?,
                );

                let mut new_artists = new_artists.lock().await;
                for link in page.select(selector) {
                    let entry: &mut (usize, usize) = new_artists
                        .entry(
                            link.text()
                                .next()
                                .ok_or_else(|| {
                                    error!("Can't find band name on link {}", link.html())
                                })?
                                .to_string(),
                        )
                        .or_default();
                    entry.0 += 1;
                    entry.1 += link
                        .value()
                        .attr("id")
                        .ok_or_else(|| error!("Can't find id on link {}", link.html()))?
                        .strip_prefix('s')
                        .ok_or_else(|| error!("Malformed id on link {}", link.html()))?
                        .parse::<usize>()
                        .map_err(|err| error!("Invalid id on link {}: {err}", link.html()))?;
                }

                Ok::<_, ()>(())
            }
        });
    }

    // we are ignoring errors here
    join_set.join().await;

    let mut artists: Vec<(String, (usize, usize))> = new_artists
        .into_inner()
        .into_iter()
        .filter(|(name, _)| {
            !existing_artists
                .iter()
                .any(|artist| artist.eq_ignore_ascii_case(name))
        })
        .collect::<Vec<_>>();
    artists.sort_unstable_by(|(_, (n1, v1)), (_, (n2, v2))| {
        n1.cmp(n2).reverse().then(
            (*v1 as f64 / *n1 as f64)
                .partial_cmp(&(*v2 as f64 / *n2 as f64))
                .expect("invalid float"),
        )
    });
    for (name, (n, v)) in artists.into_iter().take(args.num_results) {
        if args.verbose {
            println!(
                "{name} ({n} occurrencies, {} avg distance)",
                v as f64 / n as f64
            );
        } else {
            println!("{name}");
        }
    }

    Ok(())
}
