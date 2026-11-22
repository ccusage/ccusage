use std::sync::Arc;

use crate::TimestampMs;

use super::{
    CachedHit, Candidate, HitKind, Lookup, Name, NameCache, Pass, Pricing, PricingMap, PricingRows,
    PricingSchedule, RequestCache, RequestHit, Row, RowMatch, Scope, host_listing,
    host_of_model_id, is_host_segment, keys::normalized_key_bytes, last_segment,
    models_dev_pricing, names_pricing_key, normalized_pricing_key, pricing_key_matches,
    same_host_spelling, schedule_for, unlabeled_model,
};

#[derive(Clone, Copy)]
enum RowSelection {
    Any,
    Hosted,
    PaidSuffix,
    FreeDirectSuffix,
}

impl RowSelection {
    fn includes(self, row: &Row, lookup: Lookup) -> bool {
        match self {
            Self::Any => true,
            Self::Hosted => row.scope == Scope::Hosted,
            Self::PaidSuffix => {
                !row.is_free() || (matches!(lookup, Lookup::Context) && row.scope == Scope::Direct)
            }
            Self::FreeDirectSuffix => row.scope == Scope::Direct && row.is_free(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchKind {
    StrictExact,
    Exact,
    Fuzzy,
    Fallback,
}

pub struct AgentModel<'a> {
    raw: &'a str,
    display: String,
}

impl<'a> AgentModel<'a> {
    pub fn new(agent: &str, raw: &'a str) -> Self {
        Self {
            raw,
            display: format!("[{agent}] {raw}"),
        }
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn into_display(self) -> String {
        self.display
    }

    pub fn candidates(&self) -> [(&str, MatchKind); 2] {
        [
            (self.display(), MatchKind::StrictExact),
            (self.raw, MatchKind::Fuzzy),
        ]
    }

    pub fn overriding_candidate(&self, pricing: Option<&PricingMap>) -> Option<(&str, MatchKind)> {
        let pricing = pricing?;
        [self.display(), self.raw]
            .into_iter()
            .find(|model| pricing.has_user_override(model))
            .map(|model| (model, MatchKind::Fuzzy))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingRoute {
    Direct(&'static str),
    Host(&'static str),
    Foreign,
}

/// The vendor whose own API a recorded provider ID names, if any.
pub fn direct_billing_vendor(provider: &str) -> Option<&'static str> {
    super::data::DIRECT_BILLING_PROVIDERS
        .iter()
        .find_map(|(id, vendor)| (*id == provider).then_some(*vendor))
}

/// The vendor whose dated prices a recorded host follows, if any.
pub fn host_billing_vendor(provider: &str) -> Option<&'static str> {
    super::data::HOSTS_FOLLOWING_VENDOR_PRICES
        .iter()
        .find_map(|(id, vendor)| (*id == provider).then_some(*vendor))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "resolver-trace")]
pub enum ResolutionStep {
    ReportedOverride,
    AuthoritativeIdentity,
    AuthoritativeSuffix,
    ModelsDevIdentity,
    AuthoritativeFuzzy,
    ModelsDevFuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "resolver-trace")]
pub struct Explanation<'a> {
    pub candidate: &'a str,
    pub step: ResolutionStep,
    pub row_key: &'a str,
    pub source: super::Source,
}

pub struct PricedIdentity<'a> {
    pub name: std::borrow::Cow<'a, str>,
    pub handle: Option<PricingHandle>,
}

#[derive(Debug, Clone, Copy)]
pub struct PricingHandle {
    rates: Pricing,
    schedule: Option<&'static PricingSchedule>,
}

impl PricingHandle {
    fn new(row: &Row, hit: CachedHit) -> Self {
        Self {
            rates: Pricing {
                estimated: hit.estimated(),
                ..row.rates()
            },
            schedule: row
                .schedule
                .filter(|schedule| schedule.follows_row || hit.scheduled()),
        }
    }

    fn on_route(
        row: &Row,
        route: BillingRoute,
        reported: &str,
        matched: &str,
        hit: CachedHit,
    ) -> Self {
        let vendor = match route {
            BillingRoute::Direct(vendor) => Some(vendor),
            BillingRoute::Host(vendor) => (row.scope == Scope::Direct).then_some(vendor),
            BillingRoute::Foreign => None,
        };
        let schedule = match vendor {
            Some(vendor) => [reported, matched]
                .into_iter()
                .find_map(|name| {
                    let resolved = crate::model_aliases::resolve_model_name(name);
                    [resolved.as_ref(), name]
                        .into_iter()
                        .find_map(|name| schedule_for(last_segment(name)))
                })
                .or(row
                    .schedule
                    .filter(|schedule| schedule.follows_row || hit.case_fallback()))
                .filter(|schedule| schedule.vendor == vendor),
            None => None,
        };
        Self {
            rates: Pricing {
                estimated: hit.estimated(),
                ..row.rates()
            },
            schedule,
        }
    }

    pub fn is_time_dependent(&self) -> bool {
        self.schedule.is_some()
    }

    pub fn is_estimate(&self) -> bool {
        self.rates.estimated
    }

    pub fn with_rates_at<T>(
        &self,
        timestamp: Option<TimestampMs>,
        calculate: impl FnOnce(&Pricing) -> T,
    ) -> T {
        match (timestamp, self.schedule) {
            (Some(timestamp), Some(schedule)) => calculate(&self.rates.at(schedule, timestamp)),
            _ => calculate(&self.rates),
        }
    }
}

impl PricingMap {
    pub fn find(&self, model: &str) -> Option<Pricing> {
        let (_, found, hit) = self.resolve_row_with(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(Pricing {
            estimated: hit.estimated(),
            ..found.row.rates()
        })
    }

    #[cfg(feature = "resolver-trace")]
    pub fn explain<'a>(
        &'a self,
        reported_model: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
    ) -> Option<Explanation<'a>> {
        let (candidate, found, hit) = self.resolve_row_with(
            reported_model,
            candidates,
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(Explanation {
            candidate,
            step: hit.step()?,
            row_key: found.key,
            source: found.row.source,
        })
    }

    pub fn resolve_at<'a>(
        &self,
        reported_model: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
        timestamp: Option<TimestampMs>,
    ) -> Option<(&'a str, Pricing)> {
        let (model, found, hit) = self.resolve_row_with(
            reported_model,
            candidates,
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        let handle = PricingHandle::new(found.row, hit);
        Some((model, handle.with_rates_at(timestamp, |rates| *rates)))
    }

    pub(crate) fn resolve_on_route_at<'a>(
        &self,
        reported_model: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
        route: BillingRoute,
        timestamp: Option<TimestampMs>,
    ) -> Option<(&'a str, Pricing)> {
        let lookup = match route {
            BillingRoute::Direct(_) => Lookup::DirectPricing,
            BillingRoute::Host(_) | BillingRoute::Foreign => Lookup::Pricing,
        };
        let (model, found, hit) =
            self.resolve_row_with(reported_model, candidates, lookup, models_dev_pricing)?;
        let handle = PricingHandle::on_route(
            found.row,
            route,
            reported_model.unwrap_or(model),
            model,
            hit,
        );
        Some((model, handle.with_rates_at(timestamp, |rates| *rates)))
    }

    /// Resolves `logged` once and returns that row's handle with a model id
    /// that resolves to the same row: the configured alias target when the row
    /// was found through that alias and the target itself resolves to it,
    /// otherwise `logged`. Usage grouped by the returned name therefore shares
    /// a group only when it bills the same row.
    pub fn pricing_identity<'a>(&self, logged: &'a str) -> PricedIdentity<'a> {
        let resolve = |model: &str| {
            self.resolve_row_with(
                Some(model),
                &[(model, MatchKind::Fuzzy)],
                Lookup::Pricing,
                models_dev_pricing,
            )
            .map(|(_, found, hit)| (PricingHandle::new(found.row, hit), hit))
        };
        let resolved = resolve(logged);
        let name = resolved
            .filter(|(_, hit)| hit.aliased())
            .and_then(|(_, hit)| {
                let alias = configured_alias(logged)?;
                let same = resolve(&alias).is_some_and(|(_, own)| own.same_row(hit));
                same.then_some(alias)
            })
            .unwrap_or(std::borrow::Cow::Borrowed(logged));
        PricedIdentity {
            name,
            handle: resolved.map(|(handle, _)| handle),
        }
    }

    pub fn find_handle(&self, model: &str) -> Option<PricingHandle> {
        let (_, found, hit) = self.resolve_row_with(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(PricingHandle::new(found.row, hit))
    }

    pub(super) fn available_models_dev(&self) -> Option<&PricingRows> {
        self.live_models_dev
            .get()
            .and_then(Option::as_deref)
            .or(self.models_dev.as_deref())
    }

    fn select_models_dev<'a>(
        &self,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> Option<&PricingRows> {
        if self.enable_models_dev_fallback {
            self.live_models_dev
                .get_or_init(|| live().map(|source| Arc::clone(&source.rows)));
        }
        self.available_models_dev()
    }

    #[inline(never)]
    fn authoritative<'a>(
        &'a self,
        model: Name<'_>,
        lookup: Lookup,
        hosted: bool,
        fold: bool,
        models_dev: &dyn Fn() -> Option<&'a PricingRows>,
        selection: RowSelection,
    ) -> Option<RowMatch<'a>> {
        let project = |found: RowMatch<'a>| {
            (lookup.includes(found.row) && selection.includes(found.row, lookup)).then_some(found)
        };
        if self.overrides.is_empty() {
            return self.rows.authoritative(model, hosted, fold, project);
        }
        self.overrides
            .id_identity(model.identity())
            .and_then(|id| self.resolve_override(id, models_dev, false))
            .and_then(project)
            .or_else(|| {
                self.rows.authoritative(model, hosted, fold, |found| {
                    let found = match self.overrides.shadow(found) {
                        Some(id) => self.resolve_override(id, models_dev, false)?,
                        None => found,
                    };
                    project(found)
                })
            })
    }

    pub(super) fn resolve_row_with<'a, 'live>(
        &self,
        reported: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
        lookup: Lookup,
        live: impl FnOnce() -> Option<&'live Self>,
    ) -> Option<(&'a str, RowMatch<'_>, CachedHit)> {
        let search = || {
            let live = std::cell::Cell::new(Some(live));
            let models_dev = &|| live.take().and_then(|load| load());
            let hit = self.search_request(reported, candidates, lookup, models_dev)?;
            if matches!(lookup, Lookup::Pricing)
                && self.is_free_vendor_row(hit.row)
                && names_foreign_host(reported, candidates)
            {
                return self
                    .search_request(reported, candidates, Lookup::Paid, models_dev)
                    .map(|paid| RequestHit {
                        row: paid.row.as_estimate(),
                        ..paid
                    });
            }
            Some(hit)
        };
        let hit = match lookup {
            Lookup::Pricing | Lookup::DirectPricing => self
                .requests
                .get_or_init(RequestCache::default)
                .get_or_init(
                    reported,
                    candidates,
                    matches!(lookup, Lookup::DirectPricing),
                    search,
                ),
            Lookup::Context | Lookup::Paid => search(),
        }?;
        let model = match hit.candidate {
            Some(index) => candidates.get(index)?.0,
            None => reported?,
        };
        self.cached_row(model, hit.row)
    }

    fn is_free_vendor_row(&self, hit: CachedHit) -> bool {
        !hit.overridden()
            && !hit.is_models_dev()
            && self
                .rows
                .at(hit.id())
                .is_some_and(|found| found.row.scope == Scope::Direct && found.row.is_free())
    }

    #[inline(never)]
    fn search_request<'live>(
        &self,
        reported: Option<&str>,
        candidates: &[(&str, MatchKind)],
        lookup: Lookup,
        live: &dyn Fn() -> Option<&'live Self>,
    ) -> Option<RequestHit> {
        let lookup = if matches!(lookup, Lookup::Pricing)
            && let Some(model) =
                reported.filter(|model| model.contains('.') && !model.contains('/'))
            && candidates.iter().any(|&(candidate, kind)| {
                matches!(kind, MatchKind::Exact | MatchKind::StrictExact)
                    && candidate.split_once('/').is_some_and(|(provider, id)| {
                        id == model
                            && (direct_billing_vendor(provider).is_some()
                                || !is_host_segment(provider))
                    })
            }) {
            Lookup::DirectPricing
        } else {
            lookup
        };
        let models_dev = || self.select_models_dev(live);
        let cache = self.find_cache.get_or_init(NameCache::default);
        let matching = |name: &str, pass| {
            cache.get_or_init(name, lookup, pass, |slot| {
                self.search_pass(name, slot, lookup, pass, &models_dev)
            })
        };
        if !self.overrides.is_empty() {
            let reported = reported.map(|model| (None, model));
            let requested = candidates
                .iter()
                .enumerate()
                .filter(|(_, (_, kind))| *kind != MatchKind::Fallback)
                .map(|(index, &(model, _))| (Some(index), model));
            if let Some(hit) =
                reported
                    .into_iter()
                    .chain(requested)
                    .find_map(|(candidate, model)| {
                        matching(model, Pass::Label).map(|row| RequestHit { candidate, row })
                    })
            {
                return Some(hit);
            }
        }
        if let Some(alias) = reported.and_then(configured_alias)
            && let Some(row) = self.identity_hit(alias.as_ref(), lookup, false, &models_dev)
        {
            return Some(RequestHit {
                candidate: None,
                row: row.through_alias(true),
            });
        }
        let has_fallback = candidates
            .iter()
            .any(|&(_, kind)| kind == MatchKind::Fallback);
        for fallback in [false, true] {
            if fallback && !has_fallback {
                break;
            }
            let for_pass = |pass: Pass| {
                if !pass.searches(lookup) {
                    return None;
                }
                candidates
                    .iter()
                    .enumerate()
                    .find_map(|(index, &(model, kind))| {
                        ((kind == MatchKind::Fallback) == fallback
                            && pass.allows(model, kind, reported))
                        .then(|| matching(model, pass))
                        .flatten()
                        .map(|row| RequestHit {
                            candidate: Some(index),
                            row,
                        })
                    })
            };
            let host = |qualified, fold| {
                self.search_host_candidates(
                    reported,
                    candidates,
                    lookup,
                    fallback,
                    cache,
                    &models_dev,
                    qualified,
                    fold,
                )
            };
            if let Some(hit) = host(true, false)
                .or_else(|| host(true, true))
                .or_else(|| host(false, false))
                .or_else(|| Pass::IDENTITY.into_iter().find_map(for_pass))
                .or_else(|| {
                    self.search_case_fallback(
                        reported,
                        candidates,
                        lookup,
                        fallback,
                        cache,
                        &models_dev,
                    )
                    .or_else(|| for_pass(Pass::S56))
                })
            {
                return Some(hit);
            }
        }
        None
    }

    #[inline(never)]
    #[expect(
        clippy::too_many_arguments,
        reason = "The host scan uses the same request state as the identity passes."
    )]
    fn search_host_candidates<'a>(
        &'a self,
        reported: Option<&str>,
        candidates: &[(&str, MatchKind)],
        lookup: Lookup,
        fallback: bool,
        cache: &NameCache,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
        qualified: bool,
        fold: bool,
    ) -> Option<RequestHit> {
        candidates
            .iter()
            .enumerate()
            .find_map(|(index, &(model, kind))| {
                if (kind == MatchKind::Fallback) != fallback
                    || model.contains('/') != qualified
                    || !Pass::Host.allows(model, kind, reported)
                {
                    return None;
                }
                let row = if fold {
                    self.search_host(model, &cache.candidate(), lookup, true, models_dev)?
                        .through_case_fallback()
                } else {
                    cache.get_or_init(model, lookup, Pass::Host, |slot| {
                        self.search_host(model, slot, lookup, false, models_dev)
                    })?
                };
                Some(RequestHit {
                    candidate: Some(index),
                    row,
                })
            })
    }

    #[inline(never)]
    fn search_case_fallback<'a>(
        &'a self,
        reported: Option<&str>,
        candidates: &[(&str, MatchKind)],
        lookup: Lookup,
        fallback: bool,
        cache: &NameCache,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<RequestHit> {
        if !fallback
            && let Some(alias) = reported.and_then(configured_alias)
            && let Some(row) = self.identity_hit(alias.as_ref(), lookup, true, models_dev)
        {
            return Some(RequestHit {
                candidate: None,
                row: row.through_alias(true).through_case_fallback(),
            });
        }
        if let Some(hit) = self.search_host_candidates(
            reported, candidates, lookup, fallback, cache, models_dev, false, true,
        ) {
            return Some(hit);
        }
        for pass in Pass::IDENTITY {
            if !pass.searches(lookup) {
                continue;
            }
            for (index, &(model, kind)) in candidates.iter().enumerate() {
                if (kind == MatchKind::Fallback) == fallback
                    && pass.allows(model, kind, reported)
                    && let Some(row) = self.search_identity(
                        model,
                        &cache.candidate(),
                        lookup,
                        pass,
                        true,
                        models_dev,
                    )
                {
                    return Some(RequestHit {
                        candidate: Some(index),
                        row: row.through_case_fallback(),
                    });
                }
            }
        }
        None
    }

    fn models_dev_names<'a, 'n>(
        &'a self,
        name: Name<'n>,
        lookup: Lookup,
        fold: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> impl Iterator<Item = Name<'n>> {
        // NOTE(2026-11-20, #2238): An authoritative row for an id behind the path outranks the models.dev row for it. After S3 only a vendor's free row can be left there, and it must still answer through S3f.
        let unclaimed = move |suffix: &Name<'_>| {
            self.authoritative(*suffix, lookup, false, fold, models_dev, RowSelection::Any)
                .is_none()
        };
        std::iter::once(name).chain(name.suffixes().take_while(unclaimed))
    }

    fn identity_hit<'a>(
        &'a self,
        model: &str,
        lookup: Lookup,
        fold: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        let slot = Candidate::default();
        slot.own_names(model, |name| {
            self.authoritative(name, lookup, true, fold, models_dev, RowSelection::Any)
        })
        .map(|found| CachedHit::new(found, HitKind::AuthoritativeIdentity))
        .or_else(|| {
            let rows = models_dev()?;
            slot.own_names(model, |name| rows.exact(name, lookup, fold))
                .map(|found| CachedHit::new(found, HitKind::ModelsDevIdentity))
        })
    }

    fn search_identity<'a>(
        &'a self,
        model: &str,
        slot: &Candidate<'_>,
        lookup: Lookup,
        pass: Pass,
        fold: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        match pass {
            Pass::S2 => slot
                .names_marked(model, |name| {
                    self.authoritative(name, lookup, true, fold, models_dev, RowSelection::Any)
                })
                .map(|(found, aliased)| {
                    CachedHit::new(found, HitKind::AuthoritativeIdentity).through_alias(aliased)
                }),
            Pass::S34 | Pass::S3f => {
                let free = matches!(pass, Pass::S3f);
                if free && matches!(lookup, Lookup::Context) {
                    return None;
                }
                let selection = if free {
                    RowSelection::FreeDirectSuffix
                } else {
                    RowSelection::PaidSuffix
                };
                slot.names_marked(model, |name| {
                    name.suffixes().find_map(|suffix| {
                        self.authoritative(suffix, lookup, false, fold, models_dev, selection)
                    })
                })
                .map(|(found, aliased)| {
                    CachedHit::new(found, HitKind::AuthoritativeSuffix).through_alias(aliased)
                })
                .or_else(|| {
                    if free {
                        return None;
                    }
                    let rows = models_dev()?;
                    slot.names_marked(model, |name| {
                        self.models_dev_names(name, lookup, fold, models_dev)
                            .find_map(|name| rows.exact(name, lookup, fold))
                    })
                    .map(|(found, aliased)| {
                        CachedHit::new(found, HitKind::ModelsDevIdentity).through_alias(aliased)
                    })
                })
            }
            Pass::Host | Pass::Label | Pass::S56 => unreachable!(),
        }
    }

    fn search_pass<'a>(
        &'a self,
        model: &str,
        slot: &Candidate<'_>,
        lookup: Lookup,
        pass: Pass,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        match pass {
            Pass::Label => {
                let id = self.overrides.label_id(model, slot)?;
                let found = self.resolve_override(id, models_dev, false)?;
                lookup
                    .includes(found.row)
                    .then(|| CachedHit::new(found, HitKind::ReportedOverride))
            }
            Pass::S2 | Pass::S34 | Pass::S3f => {
                self.search_identity(model, slot, lookup, pass, false, models_dev)
            }
            Pass::S56 => {
                let secondary = models_dev();
                if let Some(rows) = secondary
                    && slot
                        .names(model, |name| {
                            self.models_dev_names(name, lookup, false, models_dev)
                                .any(|name| {
                                    rows.contains_exact_only_identity(name.raw(), name.identity())
                                })
                                .then_some(())
                        })
                        .is_some()
                {
                    return None;
                }
                slot.names_marked(model, |name| {
                    self.search_fuzzy(name, lookup, secondary, models_dev)
                })
                .map(|(hit, aliased)| hit.through_alias(aliased))
            }
            Pass::Host => unreachable!(),
        }
    }

    // NOTE(2026-11-21, #2229): When an agent names the host that served a request, in the logged id or as a logged-provider candidate (Exact or StrictExact), or logs a Bedrock-spelled id, the host's own listing bills it ahead of the vendor's list price found through another candidate. A direct vendor in front of the id names no host. Only LiteLLM keys a listing by its host, so the pass reads LiteLLM rows keyed by that path, which models.dev's `owner/model` ids are not.
    #[inline(never)]
    fn search_host<'a>(
        &'a self,
        model: &str,
        slot: &Candidate<'_>,
        lookup: Lookup,
        fold: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        let name = slot.name(model);
        let listed = match model.split_once('/') {
            Some((segment, _)) => {
                if !is_host_segment(segment) {
                    return None;
                }
                match host_listing(segment) {
                    Some((host, listing)) => self.host_listing_row(
                        name.suffixes().next()?,
                        host,
                        listing,
                        lookup,
                        fold,
                        models_dev,
                    ),
                    None => self.authoritative(
                        name,
                        lookup,
                        false,
                        fold,
                        models_dev,
                        RowSelection::Hosted,
                    ),
                }
            }
            None => {
                let (host, listing) = host_of_model_id(model)?;
                self.host_listing_row(name, host, listing, lookup, fold, models_dev)
            }
        };
        listed.map(|found| CachedHit::new(found, HitKind::AuthoritativeIdentity))
    }

    // NOTE(2026-11-21, #2229): A host's default-region listing is its bare row, then its prefixed rows. A regional path missing from the catalogue, such as an offline `bedrock/us-east-1/<id>`, bills the host's default-region listing rather than falling to the vendor's price.
    fn host_listing_row<'a>(
        &'a self,
        id: Name<'_>,
        host: u8,
        listing: &super::data::HostListing,
        lookup: Lookup,
        fold: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<RowMatch<'a>> {
        self.rows
            .host_row(id, host, lookup, fold)
            .and_then(|found| match self.overrides.shadow(found) {
                Some(id) => self.resolve_override(id, models_dev, false),
                None => Some(found),
            })
            .or_else(|| {
                listing.prefixes.iter().find_map(|prefix| {
                    let key = format!("{prefix}/{}", id.raw());
                    let identity = std::sync::OnceLock::new();
                    self.authoritative(
                        Name::new(&key, &identity),
                        lookup,
                        false,
                        fold,
                        models_dev,
                        RowSelection::Hosted,
                    )
                })
            })
            .or_else(|| {
                let default_region = id.suffixes().next()?;
                self.host_listing_row(default_region, host, listing, lookup, fold, models_dev)
            })
    }

    pub(super) fn search_fuzzy<'a>(
        &'a self,
        name: Name<'_>,
        lookup: Lookup,
        secondary: Option<&'a PricingRows>,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        self.search_fuzzy_with_keys(name, lookup, secondary, models_dev, PricingRows::keys)
    }

    pub(super) fn search_fuzzy_with_keys<'a, I>(
        &'a self,
        name: Name<'_>,
        lookup: Lookup,
        secondary: Option<&'a PricingRows>,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
        keys: impl Fn(&'a PricingRows) -> I,
    ) -> Option<CachedHit>
    where
        I: Iterator<Item = (usize, &'a str)>,
    {
        let normalized = name.identity();
        let matches = |key| pricing_key_matches(key, name.raw(), normalized);
        let base = keys(&self.rows).filter(|&(_, key)| matches(key));
        let resolve = |(id, key)| Some((key, self.rows.at(id)?));
        let primary = if self.overrides.is_empty() {
            Self::fuzzy(base.filter_map(resolve), name.raw(), normalized, lookup)
        } else {
            let shadowed = self.override_shadows.get_or_init(|| {
                self.rows
                    .keys()
                    .filter_map(|(id, key)| {
                        self.overrides
                            .shadow_identity(&normalized_pricing_key(key), || {
                                self.rows.vendor_id(id).map(str::len)
                            })
                    })
                    .collect()
            });
            let resolve_override = |id| self.resolve_override(id, models_dev, true);
            let base = base.filter_map(resolve).filter_map(|(key, raw)| {
                let found = match self
                    .overrides
                    .shadow_identity(&normalized_pricing_key(key), || {
                        raw.row.vendor_id().map(str::len)
                    }) {
                    Some(id) => resolve_override(id)?,
                    None => raw,
                };
                Some((key, found))
            });
            let overrides = self
                .overrides
                .names()
                .enumerate()
                .filter_map(|(id, (key, _))| {
                    if shadowed.contains(&id) || !matches(key) {
                        return None;
                    }
                    Some((key, resolve_override(id)?))
                });
            Self::fuzzy(base.chain(overrides), name.raw(), normalized, lookup)
        };
        primary
            .map(|found| CachedHit::new(found, HitKind::AuthoritativeFuzzy))
            .or_else(|| {
                let secondary = secondary.filter(|rows| rows.may_match_fuzzy(name.raw()))?;
                Self::fuzzy(
                    keys(secondary)
                        .filter(|&(_, key)| matches(key))
                        .filter_map(|(id, key)| Some((key, secondary.at(id)?))),
                    name.raw(),
                    normalized,
                    lookup,
                )
                .map(|found| CachedHit::new(found, HitKind::ModelsDevFuzzy))
            })
    }

    fn cached_row<'a>(
        &self,
        name: &'a str,
        hit: CachedHit,
    ) -> Option<(&'a str, RowMatch<'_>, CachedHit)> {
        let row = if hit.overridden() {
            self.overrides.resolved(hit.id())
        } else if hit.is_models_dev() {
            self.available_models_dev()?.at(hit.id())
        } else {
            self.rows.at(hit.id())
        }?;
        Some((name, row, hit))
    }

    fn fuzzy<'a>(
        rows: impl Iterator<Item = (&'a str, RowMatch<'a>)>,
        name: &str,
        normalized: &str,
        lookup: Lookup,
    ) -> Option<RowMatch<'a>> {
        let segment = last_segment(normalized);
        let pool = rows.filter(|(key, found)| {
            lookup.includes(found.row)
                && (!found.row.is_free()
                    || (matches!(lookup, Lookup::Context) && found.row.scope == Scope::Direct))
                && (!found.row.exact_only || names_pricing_key(key, name, normalized))
        });
        let mut best = None;
        let mut best_not_longer = None;
        let mut equal_segment_seen = false;
        for (key, found) in pool {
            let row_segment = last_segment(key);
            let rank = (key.len(), std::cmp::Reverse(key));
            if best.is_none_or(|(previous, _)| rank > previous) {
                best = Some((rank, found));
            }
            if row_segment.len() <= segment.len() {
                if best_not_longer.is_none_or(|(previous, _)| rank > previous) {
                    best_not_longer = Some((rank, found));
                }
                equal_segment_seen |= normalized_key_bytes(row_segment).eq(segment.bytes());
            }
        }
        let winner = if equal_segment_seen {
            best_not_longer
        } else {
            best
        };
        winner.map(|(_, found)| found)
    }

    pub(super) fn has_user_override(&self, model: &str) -> bool {
        self.overrides
            .label_id(model, &Candidate::default())
            .is_some()
    }

    pub fn context_limit(&self, model: &str) -> Option<u64> {
        self.resolve_context_limit(model, MatchKind::Fuzzy, models_dev_pricing)
    }

    pub(super) fn resolve_context_limit<'live>(
        &self,
        model: &str,
        kind: MatchKind,
        live: impl FnOnce() -> Option<&'live Self>,
    ) -> Option<u64> {
        let candidates = [(model, kind)];
        let live = std::cell::Cell::new(Some(live));
        let limit = |lookup| {
            self.resolve_row_with(Some(model), &candidates, lookup, || {
                live.take().and_then(|load| load())
            })
            .and_then(|(_, found, _)| found.row.context_limit)
        };
        limit(Lookup::Pricing).or_else(|| limit(Lookup::Context))
    }
}

fn configured_alias(label: &str) -> Option<std::borrow::Cow<'_, str>> {
    [label, super::unlabeled_model(label)]
        .into_iter()
        .map(|model| (model, crate::model_aliases::resolve_model_name(model)))
        .find_map(|(model, alias)| (alias != model).then_some(alias))
}

// NOTE(2026-11-21, #2228): The owner decided that a vendor's free listing reached through a paid host with no listing of its own bills an estimate from the closest paid listing of the same model, because under-reporting is worse than an estimate. A host counts only when the agent logged it (the logged id itself, or an Exact or StrictExact provider candidate), and only a host known to charge counts: a shared host listing or a LiteLLM provider with a paid row. An agent's `[name] ` display label, a direct vendor, a model owner, a vendor's own plan, and a local or unknown provider such as `ollama` or `vllm` name no paid host, so their free usage stays free.
fn names_foreign_host(reported: Option<&str>, candidates: &[(&str, MatchKind)]) -> bool {
    candidates
        .iter()
        .filter(|&&(name, kind)| {
            Some(name) == reported || matches!(kind, MatchKind::Exact | MatchKind::StrictExact)
        })
        .map(|&(name, _)| name)
        .chain(reported)
        .filter_map(|name| {
            unlabeled_model(name)
                .split_once('/')
                .map(|(segment, _)| segment)
        })
        .any(is_paid_host)
}

fn is_paid_host(segment: &str) -> bool {
    !super::models_dev::is_vendor_segment(segment)
        && (host_listing(segment).is_some()
            || super::embedded::litellm_sources().is_some_and(|sources| {
                sources
                    .paid_host_providers
                    .iter()
                    .any(|provider| same_host_spelling(provider, segment))
            }))
}
