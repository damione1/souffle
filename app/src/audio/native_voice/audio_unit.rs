//! AVFAudio's macOS getter uses the Carbon AudioUnit spelling, while objc2's
//! generated AudioToolbox binding uses the opaque spelling. Both are borrowed
//! pointers; keep the signature check and adapt only these two known spellings.
use objc2::{
    encode::{Encode, Encoding},
    msg_send,
    runtime::{AnyClass, Sel},
    sel,
};
use objc2_audio_toolbox::AudioUnit;
use objc2_avf_audio::AVAudioIONode;

use super::Result;

#[repr(transparent)]
struct CarbonAudioUnit(AudioUnit);

// SAFETY: AudioComponent.h typedefs the macOS handle to
// ComponentInstanceRecord *, whose sole field is long data[1] (Components.h).
// This transparent wrapper retains the pointer ABI. It is never dereferenced.
unsafe impl Encode for CarbonAudioUnit {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct(
        "ComponentInstanceRecord",
        &[Encoding::Array(1, &<std::ffi::c_long>::ENCODING)],
    ));
}

enum UnitEncoding {
    Carbon,
    Opaque,
}

fn checked_encoding(class: &AnyClass, selector: Sel) -> Result<UnitEncoding> {
    if class.verify_sel::<(), CarbonAudioUnit>(selector).is_ok() {
        return Ok(UnitEncoding::Carbon);
    }
    if class.verify_sel::<(), AudioUnit>(selector).is_ok() {
        return Ok(UnitEncoding::Opaque);
    }
    Err("Unsupported AVAudioIONode audioUnit method signature".into())
}

/// Borrow the node's handle without changing ownership or disabling objc2's
/// encoding assertions. Signature verification also runs in release builds.
pub(super) fn audio_unit(node: &AVAudioIONode) -> Result<AudioUnit> {
    match checked_encoding(node.class(), sel!(audioUnit))? {
        UnitEncoding::Carbon => {
            // SAFETY: The complete no-argument method signature was verified
            // above. Both SDK typedefs represent the same borrowed AudioUnit.
            let unit: CarbonAudioUnit = unsafe { msg_send![node, audioUnit] };
            Ok(unit.0)
        }
        UnitEncoding::Opaque => {
            // SAFETY: The generated binding's signature was verified above.
            Ok(unsafe { node.audioUnit() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;
    use objc2_foundation::NSObject;

    #[test]
    fn carbon_handle_matches_desktop_sdk_and_preserves_pointer_layout() {
        assert!(CarbonAudioUnit::ENCODING.equivalent_to_str("^{ComponentInstanceRecord=[1q]}"));
        assert_eq!(size_of::<CarbonAudioUnit>(), size_of::<AudioUnit>());
        assert_eq!(align_of::<CarbonAudioUnit>(), align_of::<AudioUnit>());
    }

    #[test]
    fn rejects_unrelated_returns_arguments_and_missing_methods() {
        let class = NSObject::class();
        for selector in [
            sel!(hash),
            sel!(description),
            sel!(isEqual:),
            sel!(souffleMissingAudioUnit),
        ] {
            assert!(checked_encoding(class, selector).is_err());
        }
    }

    /// Explicit hardware smoke: instantiate stopped nodes and query handles.
    /// No taps, voice processing, engine start, recording, or app bootstrap.
    #[cfg(debug_assertions)]
    #[test]
    #[ignore = "requires macOS audio hardware; run explicitly in a debug build"]
    fn stopped_av_audio_engine_getters_pass_debug_encoding_checks() {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: Nodes remain owned by the stopped engine for this scope.
            unsafe {
                let engine = objc2_avf_audio::AVAudioEngine::new();
                assert!(!engine.isRunning());
                let input = engine.inputNode();
                let output = engine.outputNode();
                audio_unit(&input).expect("input audioUnit signature must be supported");
                audio_unit(&output).expect("output audioUnit signature must be supported");
                assert!(!engine.isRunning());
            }
        });
    }
}
