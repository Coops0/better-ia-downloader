use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct FilesXml {
    #[serde(rename = "file", default)]
    pub files: Vec<FileMeta>
}

#[derive(Debug, Deserialize)]
pub struct FileMeta {
    pub size: Option<u64>,

    #[serde(rename = "md5")]
    pub md5: Option<String>,

    #[serde(rename = "crc32")]
    pub crc32: Option<String>,

    #[serde(rename = "sha1")]
    pub sha1: Option<String>,

    #[serde(rename = "@name")]
    pub name: String,

    #[serde(rename = "@source")]
    pub source: Source
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
