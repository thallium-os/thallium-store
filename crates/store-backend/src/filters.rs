//! Catalog result filters driven by the `sources.*` toggles and
//! `show_proprietary`.

use crate::settings::Settings;
use store_core::{CanonicalApp, SourceKind};

fn source_enabled(settings: &Settings, source: SourceKind) -> bool {
    match source {
        SourceKind::System => settings.sources.apt,
        SourceKind::Flathub => settings.sources.flathub,
        SourceKind::Github => settings.sources.github,
        SourceKind::Appimage => settings.sources.appimage,
    }
}

/// Unknown licenses count as free: hiding apps because a catalog lacks
/// metadata would empty the store.
fn is_proprietary(app: &CanonicalApp) -> bool {
    app.license.as_deref().is_some_and(|license| {
        let license = license.to_ascii_lowercase();
        license.contains("proprietary") || license.contains("unfree")
    })
}

/// Drop variants from disabled sources, then apps left with none (or
/// proprietary ones when hidden). Keeps `recommended_variant_id` valid.
pub fn apply(apps: &mut Vec<CanonicalApp>, settings: &Settings) {
    for app in apps.iter_mut() {
        app.variants
            .retain(|variant| source_enabled(settings, variant.source));
        let recommended_ok = app
            .recommended_variant_id
            .as_ref()
            .is_some_and(|id| app.variants.iter().any(|v| &v.id == id));
        if !recommended_ok {
            app.recommended_variant_id = app.variants.first().map(|v| v.id.clone());
        }
    }
    apps.retain(|app| {
        !app.variants.is_empty() && (settings.show_proprietary || !is_proprietary(app))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use store_core::{AppVariant, TrustLevel};

    fn variant(id: &str, source: SourceKind) -> AppVariant {
        AppVariant {
            id: id.into(),
            source,
            package_id: id.into(),
            version: None,
            download_size: None,
            installed_size: None,
            architecture: None,
            repository: None,
            install_location: None,
            trust: TrustLevel::Sandboxed,
            verified: false,
            command_preview: String::new(),
            ranking_reasons: Vec::new(),
        }
    }

    fn app(name: &str, license: Option<&str>, variants: Vec<AppVariant>) -> CanonicalApp {
        CanonicalApp {
            id: name.into(),
            name: name.into(),
            summary: String::new(),
            description: None,
            developer: None,
            homepage: None,
            license: license.map(str::to_string),
            repository: None,
            rating: None,
            fork: false,
            tags: Vec::new(),
            icon: None,
            screenshots: Vec::new(),
            last_updated: None,
            installed: false,
            launch_desktop_id: None,
            recommended_variant_id: variants.first().map(|v| v.id.clone()),
            variants,
            merge_confidence: 1.0,
            merge_evidence: Vec::new(),
            languages: Vec::new(),
        }
    }

    #[test]
    fn disabled_source_variants_and_empty_apps_are_dropped() {
        let mut settings = Settings::default();
        settings.sources.flathub = false;
        let mut apps = vec![
            app(
                "both",
                None,
                vec![
                    variant("fh", SourceKind::Flathub),
                    variant("apt", SourceKind::System),
                ],
            ),
            app("flat-only", None, vec![variant("fh2", SourceKind::Flathub)]),
        ];
        apply(&mut apps, &settings);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].variants.len(), 1);
        assert_eq!(apps[0].recommended_variant_id.as_deref(), Some("apt"));
    }

    #[test]
    fn proprietary_hidden_only_when_asked() {
        let mut settings = Settings::default();
        let make = || {
            vec![
                app(
                    "closed",
                    Some("LicenseRef-proprietary"),
                    vec![variant("a", SourceKind::System)],
                ),
                app(
                    "open",
                    Some("GPL-3.0"),
                    vec![variant("b", SourceKind::System)],
                ),
                app("unknown", None, vec![variant("c", SourceKind::System)]),
            ]
        };
        let mut shown = make();
        apply(&mut shown, &settings);
        assert_eq!(shown.len(), 3);
        settings.show_proprietary = false;
        let mut hidden = make();
        apply(&mut hidden, &settings);
        let names: Vec<_> = hidden.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["open", "unknown"]);
    }
}
