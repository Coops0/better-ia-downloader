use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct FilesXml {
    #[serde(rename = "file", default)]
    pub files: Vec<File>
}

#[derive(Debug, Deserialize)]
pub struct File {
    // #[serde(rename = "@format")]
    // pub format: Option<Format>,

    // #[serde(rename = "@original")]
    // pub original: Option<String>,

    // #[serde(rename = "@mtime")]
    // pub epoch_creation_date: Option<u64>,

    #[serde(rename = "size")]
    pub size: Option<u64>,

    #[serde(rename = "@md5")]
    pub md5: Option<String>,

    #[serde(rename = "@crc32")]
    pub crc32: Option<String>,

    #[serde(rename = "@sha1")]
    pub sha1: Option<String>,

    #[serde(rename = "@name")]
    pub name: String,

    #[serde(rename = "@source")]
    pub source: Source
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[serde(rename = "Archive BitTorrent")]
    ArchiveBitTorrent,

    #[serde(rename = "h.264 IA")]
    H264Ia,

    #[serde(rename = "Item Tile")]
    ItemTile,

    #[serde(rename = "Metadata")]
    Metadata,

    #[serde(rename = "MPEG4")]
    Mpeg4,

    #[serde(rename = "Thumbnail")]
    Thumbnail,

    #[serde(other)]
    Other
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Derivative,
    Metadata,
    Original,
    #[serde(other)]
    Other
}