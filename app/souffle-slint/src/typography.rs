//! Verify the renderer's shared font collection before showing any layout.
use crate::{MainWindow, NumericGlyph, TypefaceWeight, Typography};
fn weight_from_ui(weight: TypefaceWeight) -> souffle_typography::Weight {
    use souffle_typography::Weight;
    match weight {
        TypefaceWeight::Thin => Weight::Thin,
        TypefaceWeight::ExtraLight => Weight::ExtraLight,
        TypefaceWeight::Light => Weight::Light,
        TypefaceWeight::Regular => Weight::Regular,
        TypefaceWeight::Medium => Weight::Medium,
        TypefaceWeight::SemiBold => Weight::SemiBold,
        TypefaceWeight::Bold => Weight::Bold,
        TypefaceWeight::ExtraBold => Weight::ExtraBold,
        TypefaceWeight::Black => Weight::Black,
    }
}
use slint::fontique_011::fontique;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use souffle_typography::{FONTS, REFERENCE_GLYPHS};

pub fn verify_collection(collection: &mut fontique::Collection) -> Result<Vec<String>, String> {
    let mut cache = fontique::SourceCache::default();
    let mut evidence = Vec::new();
    for asset in FONTS {
        let mut query = collection.query(&mut cache);
        query.set_families([souffle_typography::FAMILY]);
        query.set_attributes(fontique::Attributes {
            weight: fontique::FontWeight::new(f32::from(asset.weight.number())),
            style: if asset.slant.italic() {
                fontique::FontStyle::Italic
            } else {
                fontique::FontStyle::Normal
            },
            ..Default::default()
        });
        let mut selected = None;
        query.matches_with(|font| {
            selected = Some(font.clone());
            fontique::QueryStatus::Stop
        });
        drop(query);
        let font = selected
            .ok_or_else(|| format!("{}: Inter font resolution returned no face", asset.file))?;
        let family = collection
            .family(font.family.0)
            .ok_or_else(|| format!("{}: resolved family disappeared", asset.file))?;
        let embedded = match family.fonts()[font.family.1].source().kind() {
            fontique::SourceKind::Memory(_) => true,
            fontique::SourceKind::Path(_) => false,
        };
        if !embedded
            || souffle_typography::digest(font.blob.as_ref()) != asset.sha256
            || font.synthesis.embolden()
            || font.synthesis.skew().is_some()
        {
            return Err(format!(
                "{}: Inter resolved another source or simulated a variant",
                asset.file
            ));
        }
        let charmap = font
            .charmap()
            .ok_or_else(|| format!("{}: missing character map", asset.file))?;
        for ch in REFERENCE_GLYPHS.chars().filter(|ch| !ch.is_whitespace()) {
            if charmap.map(ch).is_none() {
                return Err(format!("{}: missing glyph {ch}", asset.file));
            }
        }
        let resolved = souffle_typography::internal_name(
            font.blob.as_ref(),
            souffle_typography::POST_SCRIPT_NAME,
        )?;
        evidence.push(format!("{} | family={} | weight={} | italic={} | resolved={} | sha256={} | synthesis=none | source=embedded",
            asset.file, souffle_typography::FAMILY, asset.weight.number(), asset.slant.italic(), resolved, asset.sha256));
    }
    Ok(evidence)
}
pub fn initialize(window: &MainWindow) -> Result<Vec<String>, String> {
    let proof = verify_collection(&mut slint::fontique_011::shared_collection())?;
    window
        .global::<Typography>()
        .on_numeric_cells(|text, style| {
            ModelRc::new(VecModel::from(
                souffle_typography::numeric_cells(
                    &text,
                    style.size,
                    weight_from_ui(style.face_weight),
                    style.italic,
                )
                .into_iter()
                .map(|cell| NumericGlyph {
                    text: SharedString::from(cell.text),
                    width: cell.width,
                    offset: cell.offset,
                })
                .collect::<Vec<_>>(),
            ))
        });
    window
        .global::<Typography>()
        .on_numeric_width(|text, style| {
            souffle_typography::numeric_cells(
                &text,
                style.size,
                weight_from_ui(style.face_weight),
                style.italic,
            )
            .last()
            .map_or(0., |cell| cell.offset + cell.width)
        });
    Ok(proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn collection(fonts: &[&souffle_typography::FontAsset]) -> fontique::Collection {
        let mut collection = fontique::Collection::new(fontique::CollectionOptions {
            system_fonts: false,
            ..Default::default()
        });
        for asset in fonts {
            let blob = fontique::Blob::new(std::sync::Arc::new(asset.bytes.to_vec()));
            assert!(!collection.register_fonts(blob, None).is_empty());
        }
        collection
    }
    #[test]
    fn real_resolver_selects_all_eighteen_embedded_faces_without_synthesis() {
        let proof = verify_collection(&mut collection(&FONTS.iter().collect::<Vec<_>>())).unwrap();
        assert_eq!(proof.len(), FONTS.len());
        for row in proof {
            println!("{row}");
        }
    }
    #[test]
    fn missing_face_cannot_resolve_to_synthetic_or_system_font() {
        let error = verify_collection(&mut collection(&FONTS.iter().skip(1).collect::<Vec<_>>()))
            .unwrap_err();
        assert!(
            error.contains("Inter-Thin.ttf") && error.contains("simulated"),
            "{error}"
        );
    }
    #[test]
    fn invalid_registration_and_missing_family_are_explicit() {
        let mut collection = collection(&[]);
        assert!(
            collection
                .register_fonts(
                    fontique::Blob::new(std::sync::Arc::new(b"invalid".to_vec())),
                    None
                )
                .is_empty()
        );
        assert!(
            verify_collection(&mut collection)
                .unwrap_err()
                .contains("no face")
        );
    }
}
