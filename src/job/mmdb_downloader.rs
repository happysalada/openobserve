// Copyright 2023 Zinc Labs Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use crate::common::infra::config::{CONFIG, GEOIP_TABLE, MAXMIND_DB_CLIENT, MMDB_CITY_FILE_NAME};
use crate::common::meta::maxmind::MaxmindClient;
use crate::service::enrichment_table::geoip::{Geoip, GeoipConfig};
use futures::stream::StreamExt;
use reqwest::Client;
use sha256::try_digest;
use std::cmp::min;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time;

/// Update the global maxdb client object
pub async fn update_global_maxmind_client(fname: &str) {
    match MaxmindClient::new_with_path(fname) {
        Ok(maxminddb_client) => {
            let mut client = MAXMIND_DB_CLIENT.write().await;
            *client = Some(maxminddb_client);
            let mut geoip = GEOIP_TABLE.write();
            *geoip = Some(Geoip::new(GeoipConfig::default()).unwrap());
            log::info!("Successfully updated MaxmindClient");
        }
        Err(e) => log::warn!(
            "Failed to create MaxmindClient with path: {}, {}",
            fname,
            e.to_string()
        ),
    }
}

pub async fn is_digest_different(
    local_file_path: &str,
    remote_sha256sum_path: &str,
) -> Result<bool, anyhow::Error> {
    let response = reqwest::get(remote_sha256sum_path).await?;
    let remote_file_sha = response.text().await?;
    let local_file_sha = try_digest(Path::new(local_file_path)).unwrap_or_default();
    Ok(remote_file_sha.trim() != local_file_sha.trim())
}

pub async fn download_file(client: &Client, url: &str, path: &str) -> Result<(), String> {
    // Reqwest setup
    let res = client
        .get(url)
        .send()
        .await
        .or(Err(format!("Failed to GET from '{}'", &url)))?;
    let total_size = res
        .content_length()
        .ok_or(format!("Failed to get content length from '{}'", &url))?;

    // download chunks
    let mut file = File::create(path)
        .await
        .or(Err(format!("Failed to create file '{}'", path)))?;
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = item.or(Err("Error while downloading file".to_string()))?;
        file.write_all(&chunk)
            .await
            .or(Err("Error while writing to file".to_string()))?;
        let new = min(downloaded + (chunk.len() as u64), total_size);
        downloaded = new;
    }

    Ok(())
}

async fn run_download_files() {
    // send request and await response
    let client = reqwest::ClientBuilder::default().build().unwrap();
    let fname = format!("{}{}", &CONFIG.common.mmdb_data_dir, MMDB_CITY_FILE_NAME);

    let download_files =
        match is_digest_different(&fname, &CONFIG.common.mmdb_geolite_citydb_sha256_url).await {
            Ok(is_different) => is_different,
            Err(e) => {
                log::error!("Well something broke. {e}");
                false
            }
        };

    if download_files {
        match download_file(&client, &CONFIG.common.mmdb_geolite_citydb_url, &fname).await {
            Ok(()) => {
                update_global_maxmind_client(&fname).await;
            }
            Err(e) => log::error!("failed to download the files {}", e),
        }
    }
}

pub async fn run() -> Result<(), anyhow::Error> {
    log::info!("spawned");
    std::fs::create_dir_all(&CONFIG.common.mmdb_data_dir)?;
    // should run it every 24 hours
    let mut interval = time::interval(time::Duration::from_secs(
        CONFIG.common.mmdb_update_duration,
    ));

    // Try to load the existing file, in the beginning.
    let fname = format!("{}/{}", &CONFIG.common.mmdb_data_dir, MMDB_CITY_FILE_NAME);
    update_global_maxmind_client(&fname).await;

    loop {
        interval.tick().await;
        run_download_files().await;
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[tokio::test]
//     async fn test_run() {
//         run().await.unwrap();
//         assert!(true);
//     }
// }
