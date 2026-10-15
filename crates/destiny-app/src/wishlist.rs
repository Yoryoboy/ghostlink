//! Wish-list synchronization after Manifest and Clarity; never fatal to them.
pub use destiny_core::wishlist::WishlistRepository;
use destiny_core::wishlist::{
    WishlistFetch, WishlistGateway, WishlistSource, WishlistSourceRecord, parse_wishlist,
    wishlist_fingerprint,
};
use destiny_core::{AppError, ErrorCategory, Timestamp};

/// One configured source after a sync; `rolls` counts what is now stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistSourceReport {
    pub source: String,
    /// `updated`, `unchanged` or `unavailable`.
    pub status: &'static str,
    pub rolls: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistSyncReport {
    /// `ok`, `partial`, `unavailable`, `disabled` or `invalid_config`.
    pub status: &'static str,
    pub sources: Vec<WishlistSourceReport>,
}

impl WishlistSyncReport {
    /// Nothing was fetched or stored, for example without a gateway.
    pub fn unavailable() -> Self {
        Self {
            status: "unavailable",
            sources: Vec::new(),
        }
    }

    /// `GHOSTLINK_WISHLISTS` did not parse; stored lists were left as they were.
    pub fn invalid_config() -> Self {
        Self {
            status: "invalid_config",
            sources: Vec::new(),
        }
    }
}

fn unavailable_source(key: String, stored: Option<&WishlistSourceRecord>) -> WishlistSourceReport {
    WishlistSourceReport {
        source: key,
        status: "unavailable",
        rolls: stored.map_or(0, |record| record.rolls),
        skipped: stored.map_or(0, |record| record.skipped),
    }
}

/// Syncs every source in order. A failing source keeps its stored rolls; sources
/// no longer configured are removed, and an empty list disables wish lists.
pub fn sync_wishlists<G: WishlistGateway, R: WishlistRepository>(
    gateway: &mut G,
    repository: &mut R,
    sources: &[WishlistSource],
    checked_at: Timestamp,
) -> WishlistSyncReport {
    let Ok(stored) = repository.wishlist_sources() else {
        return WishlistSyncReport::unavailable();
    };
    let mut reports = Vec::with_capacity(sources.len());
    for source in sources {
        let key = source.key();
        let previous = stored.iter().find(|record| record.key == key);
        reports.push(
            sync_source(gateway, repository, source, &key, previous, checked_at)
                .unwrap_or_else(|_| unavailable_source(key, previous)),
        );
    }
    let keys: Vec<String> = sources.iter().map(WishlistSource::key).collect();
    if repository.retain_wishlist_sources(&keys).is_err() {
        return WishlistSyncReport {
            status: "unavailable",
            sources: reports,
        };
    }
    let failed = reports
        .iter()
        .filter(|report| report.status == "unavailable")
        .count();
    let status = match failed {
        _ if sources.is_empty() => "disabled",
        0 => "ok",
        failed if failed == reports.len() => "unavailable",
        _ => "partial",
    };
    WishlistSyncReport {
        status,
        sources: reports,
    }
}

fn sync_source<G: WishlistGateway, R: WishlistRepository>(
    gateway: &mut G,
    repository: &mut R,
    source: &WishlistSource,
    key: &str,
    previous: Option<&WishlistSourceRecord>,
    checked_at: Timestamp,
) -> Result<WishlistSourceReport, AppError> {
    let unchanged = |record: &WishlistSourceRecord| WishlistSourceReport {
        source: key.to_owned(),
        status: "unchanged",
        rolls: record.rolls,
        skipped: record.skipped,
    };
    match gateway.fetch(source, previous.and_then(|record| record.etag.as_deref()))? {
        WishlistFetch::NotModified => {
            let record =
                previous.ok_or_else(|| AppError::new(ErrorCategory::ProviderUnavailable))?;
            repository.mark_wishlist_checked(key, record.etag.as_deref(), checked_at)?;
            Ok(unchanged(record))
        }
        WishlistFetch::Content { text, etag } => {
            let fingerprint = wishlist_fingerprint(text.as_bytes());
            if let Some(record) = previous.filter(|record| record.fingerprint == fingerprint) {
                repository.mark_wishlist_checked(key, etag.as_deref(), checked_at)?;
                return Ok(unchanged(record));
            }
            let document = parse_wishlist(&text);
            // Voltron always has rolls: an empty parse is a broken body (an
            // error page or a truncated download), never a curator decision.
            // Storing it would also store its ETag and pin the empty list.
            if *source == WishlistSource::Voltron && document.rolls.is_empty() {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            repository.publish_wishlist(
                key,
                &document,
                etag.as_deref(),
                &fingerprint,
                checked_at,
            )?;
            Ok(WishlistSourceReport {
                source: key.to_owned(),
                status: "updated",
                rolls: document.rolls.len(),
                skipped: document.skipped,
            })
        }
    }
}

/// The runtime seam: wish lists follow a finished Manifest and Clarity sync.
/// Invalid configuration becomes a report; without a gateway every source is
/// reported unavailable with the rolls it still has stored.
pub fn sync_wishlists_after<G: WishlistGateway, R: WishlistRepository>(
    sources: Result<Vec<WishlistSource>, AppError>,
    acquire: impl FnOnce() -> Result<G, AppError>,
    repository: &mut R,
    checked_at: Timestamp,
) -> WishlistSyncReport {
    let Ok(sources) = sources else {
        return WishlistSyncReport::invalid_config();
    };
    let gateway = if sources.is_empty() {
        None
    } else {
        acquire().ok()
    };
    match gateway {
        Some(mut gateway) => sync_wishlists(&mut gateway, repository, &sources, checked_at),
        None => sync_wishlists(&mut NoGateway, repository, &sources, checked_at),
    }
}

/// Stands in when no client exists, or none is needed to disable every list.
struct NoGateway;
impl WishlistGateway for NoGateway {
    fn fetch(&mut self, _: &WishlistSource, _: Option<&str>) -> Result<WishlistFetch, AppError> {
        Err(AppError::new(ErrorCategory::ProviderUnavailable))
    }
}
