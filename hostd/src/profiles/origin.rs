use super::*;

/// Advisory backend-store scope. It is not a machine identifier or authentication.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub platform: String,
    pub store_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Namespace {
    schema_version: u8,
    store_id: String,
}

pub(super) fn load_or_create(directory: &Path) -> io::Result<Origin> {
    let path = directory.join(".origin.json");
    let open = || {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
    };
    let file = match open() {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Reuse SQLite's existing RNG dependency for an opaque namespace.
            // This value is never an authentication token or physical identity.
            let store_id: String = rusqlite::Connection::open_in_memory()
                .map_err(io::Error::other)?
                .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
                .map_err(io::Error::other)?;
            let namespace = Namespace {
                schema_version: 1,
                store_id,
            };
            let pending = directory.join(format!(
                ".pending-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let result = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&pending)?;
                file.write_all(&serde_json::to_vec(&namespace).map_err(io::Error::other)?)?;
                file.sync_all()?;
                fs::hard_link(&pending, &path)?;
                File::open(directory)?.sync_all()?;
                open()
            })();
            let _ = fs::remove_file(pending);
            result?
        }
        Err(error) => return Err(error),
    };
    private_metadata(&file.metadata()?, false)?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("profile origin exceeds 4 KiB"));
    }
    let namespace: Namespace = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if namespace.schema_version != 1
        || namespace.store_id.len() != 32
        || !namespace
            .store_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("unsupported or invalid profile origin namespace"));
    }
    Ok(Origin {
        platform: std::env::consts::OS.into(),
        store_id: namespace.store_id,
    })
}
