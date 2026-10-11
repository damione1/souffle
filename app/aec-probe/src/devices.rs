//! Read-only CoreAudio enumeration and explicit, local capture-device selection.
use crate::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use objc2_core_audio::*;
use serde::{Deserialize, Serialize};
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Input,
    Output,
}
impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
    pub fn bus(self) -> u32 {
        match self {
            Self::Input => 1,
            Self::Output => 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: u32,
    pub uid: String,
    pub name: String,
    pub nominal_rate: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    pub input: Device,
    pub output: Device,
}
#[derive(Serialize)]
pub struct Catalog {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
    pub default_input_uid: Option<String>,
    pub default_output_uid: Option<String>,
}

pub fn parse_uid(value: &str) -> std::result::Result<String, String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err("device UID must be nonempty and contain no control characters".into());
    }
    Ok(value.to_owned())
}

pub fn property<T: Default>(
    device: u32,
    selector: u32,
    scope: u32,
    element: u32,
) -> std::result::Result<T, String> {
    let mut result = T::default();
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    };
    let mut size = size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut result).cast(),
        )
    };
    if status != 0 || size != size_of::<T>() as u32 {
        return Err(format!(
            "CoreAudio property {selector:#x}: OSStatus {status}, size {size}"
        ));
    }
    Ok(result)
}
pub fn global<T: Default>(device: u32, selector: u32) -> std::result::Result<T, String> {
    property(device, selector, kAudioObjectPropertyScopeGlobal, 0)
}

fn hardware_ids() -> Result<Vec<u32>> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: 0,
    };
    let mut size = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as u32,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 || size % 4 != 0 || size > 65536 {
        return Err(format!("enumerate CoreAudio devices: OSStatus {status}, size {size}").into());
    }
    let mut ids = vec![0u32; size as usize / 4];
    if ids.is_empty() {
        return Ok(ids);
    }
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr()).unwrap().cast(),
        )
    };
    if status != 0 || size as usize != ids.len() * 4 {
        return Err(format!("read CoreAudio devices: OSStatus {status}, size {size}").into());
    }
    Ok(ids)
}

pub fn catalog() -> Result<Catalog> {
    let host = cpal::default_host();
    let ids = hardware_ids()?;
    let describe = |device: cpal::Device| -> Result<Device> {
        let uid = device.id()?.1;
        let id = ids
            .iter()
            .copied()
            .find(|&id| souffle_lib::audio::output_route::device_uid(id).is_ok_and(|u| u == uid))
            .ok_or_else(|| format!("CoreAudio device disappeared while resolving UID {uid:?}"))?;
        Ok(Device {
            id,
            uid,
            name: device.description()?.name().to_owned(),
            nominal_rate: global(id, kAudioDevicePropertyNominalSampleRate)?,
        })
    };
    Ok(Catalog {
        inputs: host
            .input_devices()?
            .map(&describe)
            .collect::<Result<_>>()?,
        outputs: host
            .output_devices()?
            .map(&describe)
            .collect::<Result<_>>()?,
        default_input_uid: host
            .default_input_device()
            .map(|d| d.id().map(|id| id.1))
            .transpose()?,
        default_output_uid: host
            .default_output_device()
            .map(|d| d.id().map(|id| id.1))
            .transpose()?,
    })
}
fn resolve(devices: &[Device], uid: &str) -> Result<Device> {
    parse_uid(uid)?;
    let mut matches = devices.iter().filter(|d| d.uid == uid);
    let device = matches
        .next()
        .ok_or_else(|| format!("requested device UID not available: {uid:?}"))?;
    if matches.next().is_some() {
        return Err(format!("ambiguous device UID: {uid:?}").into());
    }
    if !(8000.0..=192000.0).contains(&device.nominal_rate) {
        return Err(format!("unsupported current device rate: {}", device.nominal_rate).into());
    }
    Ok(device.clone())
}
impl Selection {
    pub fn resolve(input_uid: &str, output_uid: Option<&str>) -> Result<Self> {
        let catalog = catalog()?;
        let default_output = catalog
            .default_output_uid
            .as_deref()
            .ok_or("no default output")?;
        let output_uid = output_uid.unwrap_or(default_output);
        // The passive browser reference and FDAF route predicate use this route.
        // Refuse an unrelated output instead of changing the global default.
        if output_uid != default_output {
            return Err("requested output must match the current browser/system output; no global route will be changed".into());
        }
        Ok(Self {
            input: resolve(&catalog.inputs, input_uid)?,
            output: resolve(&catalog.outputs, output_uid)?,
        })
    }
    pub fn cpal_input(&self) -> Result<cpal::Device> {
        let device = cpal::default_host()
            .device_by_id(&cpal::DeviceId(
                cpal::HostId::CoreAudio,
                self.input.uid.clone(),
            ))
            .ok_or("requested CPAL input disappeared")?;
        confirm(&self.input, self.input.id, &device.id()?.1)?;
        Ok(device)
    }
}
pub fn confirm(expected: &Device, id: u32, uid: &str) -> Result<()> {
    if id != expected.id || uid != expected.uid {
        return Err(format!(
            "device readback mismatch: requested {} ({:?}, {}), got {uid:?} ({id})",
            expected.name, expected.uid, expected.id
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> Device {
        Device {
            id: 42,
            uid: "USB:camera with spaces".into(),
            name: "camera".into(),
            nominal_rate: 16000.0,
        }
    }
    #[test]
    fn uid_is_exact_and_rejects_empty_or_control_characters() {
        let d = device();
        assert_eq!(parse_uid(&d.uid).unwrap(), d.uid);
        for bad in ["", "  ", "camera\0", "camera\n"] {
            assert!(parse_uid(bad).is_err());
        }
    }
    #[test]
    fn resolution_rejects_missing_ambiguous_or_invalid_rate() {
        let d = device();
        assert_eq!(resolve(std::slice::from_ref(&d), &d.uid).unwrap().id, d.id);
        assert!(resolve(std::slice::from_ref(&d), "other").is_err());
        assert!(resolve(&[d.clone(), d.clone()], &d.uid).is_err());
        let mut invalid = d.clone();
        invalid.nominal_rate = f64::NAN;
        assert!(resolve(&[invalid], &d.uid).is_err());
    }
    #[test]
    fn readback_requires_both_device_id_and_uid() {
        let d = device();
        assert!(confirm(&d, d.id, &d.uid).is_ok());
        assert!(confirm(&d, 43, &d.uid).is_err());
        assert!(confirm(&d, d.id, "other").is_err());
        assert_eq!(Side::Input.bus(), 1);
        assert_eq!(Side::Output.bus(), 0);
    }
}
