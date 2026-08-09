use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "windows")]
const ICON_PATH: &str = "assets/app-icon/app-icon.ico";
const FONT_DIR: &str = "font";
const VERSION_STAMP: &str = ".font-version";

const FONT_SOURCES: &[FontSource] = &[
    FontSource {
        key: "lxgw-bright",
        repo: "lxgw/LxgwBright",
        tag_path: "LXGWBright/LXGWBright-Medium.ttf",
        file: "LXGWBright-Medium.ttf",
        family: "LXGW Bright",
    },
    FontSource {
        key: "lxgw-bright-code",
        repo: "lxgw/LxgwBright-Code",
        tag_path: "LxgwBrightCode/LXGWBrightCode-Regular.ttf",
        file: "LXGWBrightCode-Regular.ttf",
        family: "LXGW Bright Code",
    },
];

#[derive(Debug)]
struct FontSource {
    key: &'static str,
    repo: &'static str,
    tag_path: &'static str,
    file: &'static str,
    family: &'static str,
}

struct Versions(BTreeMap<&'static str, String>);

impl Versions {
    fn new() -> Self {
        Self(BTreeMap::new())
    }

    fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }

    fn insert(&mut self, key: &'static str, value: String) {
        self.0.insert(key, value);
    }

    fn load(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let mut versions = Self::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if let Some(source) = FONT_SOURCES.iter().find(|source| source.key == key) {
                versions.insert(source.key, value.trim().to_owned());
            }
        }
        (versions.0.len() == FONT_SOURCES.len()).then_some(versions)
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut text = String::new();
        for source in FONT_SOURCES {
            let tag = self.get(source.key).map(String::as_str).unwrap_or_default();
            text.push_str(source.key);
            text.push('=');
            text.push_str(tag);
            text.push('\n');
        }
        fs::write(path, text)
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed={FONT_DIR}");

    sync_fonts();

    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed={ICON_PATH}");
        let icon = manifest_dir().join(ICON_PATH);
        winresource::WindowsResource::new()
            .set_icon(icon.to_str().expect("application icon path must be UTF-8"))
            .compile()
            .expect("failed to embed Windows application icon");
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"))
}

fn sync_fonts() {
    let font_dir = manifest_dir().join(FONT_DIR);
    fs::create_dir_all(&font_dir).expect("failed to create font directory");

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "gongwen-assistant-build/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .expect("failed to create HTTP client");

    let versions = match latest_tags(&client) {
        Ok(versions) => versions,
        Err(error) => {
            if FONT_SOURCES
                .iter()
                .all(|source| valid_font_file(&font_dir.join(source.file), source.family))
            {
                println!(
                    "cargo:warning=LXGW font update check failed ({error}); using cached fonts"
                );
                return;
            }
            panic!("failed to check LXGW font releases and no cached font is available: {error}");
        }
    };

    let previous = Versions::load(&font_dir.join(VERSION_STAMP));
    let mut changed = false;
    let mut all_downloads_ok = true;

    for source in FONT_SOURCES {
        let destination = font_dir.join(source.file);
        let tag = versions
            .get(source.key)
            .map(String::as_str)
            .unwrap_or_default();
        let cached_is_current = previous
            .as_ref()
            .and_then(|previous| previous.get(source.key))
            .map(String::as_str)
            == Some(tag);
        if cached_is_current && valid_font_file(&destination, source.family) {
            continue;
        }

        match download(&client, source, tag) {
            Ok(bytes) => {
                write_font(&destination, &bytes, source);
                changed = true;
            }
            Err(error) if valid_font_file(&destination, source.family) => {
                println!(
                    "cargo:warning=download of {} failed ({error}); keeping cached {}",
                    source.file, source.file
                );
                all_downloads_ok = false;
            }
            Err(error) => {
                panic!("failed to download {}: {error}", source.file);
            }
        }
    }

    if changed && all_downloads_ok {
        versions
            .save(&font_dir.join(VERSION_STAMP))
            .expect("failed to write LXGW font version stamp");
    }
}

fn latest_tags(client: &reqwest::blocking::Client) -> Result<Versions, String> {
    let mut versions = Versions::new();
    for source in FONT_SOURCES {
        let url = format!("https://github.com/{}/releases/latest", source.repo);
        let response = client
            .get(&url)
            .send()
            .map_err(|error| format!("request to {url} failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("request to {url} returned {}", response.status()));
        }
        let final_path = response.url().path().to_owned();
        let tag = final_path
            .rsplit("/tag/")
            .next()
            .filter(|tag| !tag.is_empty())
            .ok_or_else(|| format!("could not find release tag in {final_path}"))?;
        versions.insert(source.key, tag.to_owned());
    }
    Ok(versions)
}

fn download(
    client: &reqwest::blocking::Client,
    source: &FontSource,
    tag: &str,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://github.com/{}/raw/refs/tags/{}/{}",
        source.repo, tag, source.tag_path
    );
    let response = client
        .get(&url)
        .send()
        .map_err(|error| format!("download of {url} failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("download of {url} returned {}", response.status()));
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("failed to read {url}: {error}"))?
        .to_vec();
    if bytes.len() < 1_000_000 {
        return Err(format!("downloaded {url} is unexpectedly small"));
    }
    Ok(bytes)
}

fn write_font(destination: &Path, bytes: &[u8], source: &FontSource) {
    if !valid_font(bytes, source.family) {
        panic!(
            "downloaded {} is not a valid {} font",
            source.file, source.family
        );
    }
    let tmp = destination.with_extension("tmp");
    fs::write(&tmp, bytes).expect("failed to write temporary font file");
    if destination.exists() {
        fs::remove_file(destination).expect("failed to replace cached font file");
    }
    fs::rename(&tmp, destination).expect("failed to move font file into place");
}

fn valid_font_file(path: &Path, family: &str) -> bool {
    fs::read(path)
        .ok()
        .is_some_and(|bytes| valid_font(&bytes, family))
}

fn valid_font(bytes: &[u8], family: &str) -> bool {
    let Ok(face) = ttf_parser::Face::parse(bytes, 0) else {
        return false;
    };
    let expected = family.replace(' ', "").to_lowercase();
    face.names().into_iter().any(|name| {
        name.to_string().is_some_and(|value| {
            let actual = value.replace(' ', "").to_lowercase();
            actual.contains(&expected)
        })
    })
}
