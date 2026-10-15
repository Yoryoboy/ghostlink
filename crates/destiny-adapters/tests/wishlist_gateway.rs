use destiny_adapters::wishlist::{VoltronTransport, WishlistAdapter, read_local_wishlist};
use destiny_core::wishlist::{WishlistFetch, WishlistGateway, WishlistSource};
use destiny_core::{AppError, ErrorCategory};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Transport {
    requested: Vec<Option<String>>,
}

impl VoltronTransport for Transport {
    fn get(&mut self, etag: Option<&str>) -> Result<WishlistFetch, AppError> {
        self.requested.push(etag.map(str::to_owned));
        Ok(match etag {
            Some(_) => WishlistFetch::NotModified,
            None => WishlistFetch::Content {
                text: "dimwishlist:item=1&perks=2\n".into(),
                etag: Some("\"v1\"".into()),
            },
        })
    }
}

struct TempFile(PathBuf);

impl TempFile {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ghostlink-wishlist-{}-{}.txt",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn voltron_goes_through_the_transport_with_the_stored_etag() {
    let mut gateway = WishlistAdapter(Transport::default());
    assert_eq!(
        gateway.fetch(&WishlistSource::Voltron, None).unwrap(),
        WishlistFetch::Content {
            text: "dimwishlist:item=1&perks=2\n".into(),
            etag: Some("\"v1\"".into()),
        }
    );
    assert_eq!(
        gateway.fetch(&WishlistSource::Voltron, Some("\"v1\"")).unwrap(),
        WishlistFetch::NotModified
    );
    assert_eq!(gateway.0.requested, vec![None, Some("\"v1\"".into())]);
}

#[test]
fn local_files_are_read_verbatim_without_the_byte_order_mark() {
    let file = TempFile::new("\u{feff}title:Mine\r\ndimwishlist:item=-5\r\n".as_bytes());
    let mut gateway = WishlistAdapter(Transport::default());
    let fetched = gateway
        .fetch(&WishlistSource::Local(file.0.clone()), Some("ignored"))
        .unwrap();
    assert_eq!(
        fetched,
        WishlistFetch::Content {
            text: "title:Mine\r\ndimwishlist:item=-5\r\n".into(),
            etag: None,
        }
    );
    assert!(gateway.0.requested.is_empty());
}

#[test]
fn unreadable_local_files_are_unavailable() {
    let invalid = TempFile::new(b"dimwishlist:item=1#notes:\xff\n");
    let missing = std::env::temp_dir().join("ghostlink-wishlist-missing-file.txt");
    for path in [invalid.0.clone(), missing, std::env::temp_dir()] {
        assert_eq!(
            read_local_wishlist(&path).unwrap_err().category(),
            ErrorCategory::ProviderUnavailable,
            "{}",
            path.display()
        );
    }
}
