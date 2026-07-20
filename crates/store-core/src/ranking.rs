use crate::{AppVariant, SourceKind, TrustLevel};

pub fn rank_variants(variants: &mut [AppVariant]) -> Option<String> {
    variants.sort_by_key(score_variant);
    variants.first_mut().map(|variant| {
        if variant.ranking_reasons.is_empty() {
            variant
                .ranking_reasons
                .push(default_reason(variant).to_string());
        }
        variant.id.clone()
    })
}

fn score_variant(variant: &AppVariant) -> u8 {
    match (variant.source, variant.verified, variant.trust) {
        (SourceKind::Flathub, true, TrustLevel::Sandboxed) => 0,
        (SourceKind::System, true, _) => 10,
        (SourceKind::Github, true, _) => 20,
        (SourceKind::Flathub, _, _) => 25,
        (SourceKind::System, _, _) => 30,
        (SourceKind::Github, _, _) => 40,
        (SourceKind::Appimage, _, _) => 50,
    }
}

fn default_reason(variant: &AppVariant) -> &'static str {
    match variant.source {
        SourceKind::Flathub if variant.verified => "sandboxed and verified",
        SourceKind::Flathub => "sandboxed Flatpak source",
        SourceKind::System => "native system package",
        SourceKind::Github if variant.verified => "curated GitHub release",
        SourceKind::Github => "unverified GitHub release",
        SourceKind::Appimage => "portable AppImage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(id: &str, source: SourceKind, trust: TrustLevel, verified: bool) -> AppVariant {
        AppVariant {
            id: id.to_string(),
            source,
            package_id: id.to_string(),
            version: None,
            download_size: None,
            installed_size: None,
            architecture: None,
            repository: None,
            install_location: None,
            trust,
            verified,
            command_preview: String::new(),
            ranking_reasons: Vec::new(),
        }
    }

    #[test]
    fn verified_flathub_wins_default_recommendation() {
        let mut variants = vec![
            variant("github", SourceKind::Github, TrustLevel::Verified, true),
            variant("system", SourceKind::System, TrustLevel::SystemAccess, true),
            variant("flathub", SourceKind::Flathub, TrustLevel::Sandboxed, true),
        ];

        assert_eq!(rank_variants(&mut variants).as_deref(), Some("flathub"));
    }
}
