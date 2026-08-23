//! NVIDIA's own management library, for the readings the kernel does not publish.
//!
//! AMD and Intel put utilisation and VRAM in `/sys/class/drm`, so the shell reads them for free. NVIDIA
//! publishes nothing there — the driver keeps its counters behind NVML — which is why the GPU card used to
//! fork `nvidia-smi` **once per reading**, on the dashboard's poll interval. That is the expensive pattern
//! this shell avoids everywhere else: a process start to answer a number.
//!
//! **Loaded at runtime, not linked**, for the same reason as [`pam`](super::pam): linking `libnvidia-ml` would
//! make an NVIDIA driver a build dependency of a shell that has to run on machines with an AMD card and no
//! NVIDIA anything. `dlopen` turns "no NVIDIA here" into a question answered once, at startup, by a library
//! that simply is not there.
//!
//! Only the six symbols a GPU card needs. NVML is a large API and none of the rest of it has a reader here.

use std::ffi::{CStr, c_char, c_int, c_uint};
use std::sync::OnceLock;

use util::deps::{self, Dep};

/// NVML's success code. Every call returns one of these and the value is only meaningful against it.
const NVML_SUCCESS: c_int = 0;

/// `NVML_TEMPERATURE_GPU` — the die, as opposed to a board sensor some cards also expose.
const TEMPERATURE_GPU: c_uint = 0;

/// Long enough for every product name NVIDIA ships; NVML's own constant is 64.
const NAME_LEN: usize = 96;

/// An opaque handle to one GPU. NVML hands these out and never explains them.
type Device = *mut std::ffi::c_void;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

struct Nvml {
    device_by_index: unsafe extern "C" fn(c_uint, *mut Device) -> c_int,
    name: unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int,
    utilization: unsafe extern "C" fn(Device, *mut Utilization) -> c_int,
    temperature: unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int,
    memory: unsafe extern "C" fn(Device, *mut Memory) -> c_int,
    /// Held so the symbols above stay mapped. Never called.
    _library: libloading::Library,
}

// The function pointers are into a library that is never unloaded, and NVML is documented thread-safe.
unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

fn load() -> Option<Nvml> {
    // SAFETY: loading a shared object runs its initialisers, and the six symbols are looked up by the
    // signatures NVML documents for them.
    unsafe {
        deps::open_library(Dep::Nvml, None, |library| {
            let init = *library.get::<unsafe extern "C" fn() -> c_int>(b"nvmlInit_v2\0")?;
            let device_by_index = *library
                .get::<unsafe extern "C" fn(c_uint, *mut Device) -> c_int>(
                    b"nvmlDeviceGetHandleByIndex_v2\0",
                )?;
            let name = *library.get::<unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int>(
                b"nvmlDeviceGetName\0",
            )?;
            let utilization = *library
                .get::<unsafe extern "C" fn(Device, *mut Utilization) -> c_int>(
                    b"nvmlDeviceGetUtilizationRates\0",
                )?;
            let temperature = *library
                .get::<unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int>(
                    b"nvmlDeviceGetTemperature\0",
                )?;
            let memory = *library.get::<unsafe extern "C" fn(Device, *mut Memory) -> c_int>(
                b"nvmlDeviceGetMemoryInfo\0",
            )?;
            // Initialised once, here, and never shut down: the library outlives the shell's interest in
            // it, and `nvmlShutdown` on a process that is exiting anyway buys nothing.
            if init() != NVML_SUCCESS {
                return Err(libloading::Error::DlOpenUnknown);
            }
            Ok(Nvml {
                device_by_index,
                name,
                utilization,
                temperature,
                memory,
                _library: library,
            })
        })
    }
}

fn nvml() -> Option<&'static Nvml> {
    NVML.get_or_init(load).as_ref()
}

/// Whether an NVIDIA GPU can be read on this machine.
pub fn available() -> bool {
    nvml().is_some_and(|nvml| unsafe {
        let mut device: Device = std::ptr::null_mut();
        (nvml.device_by_index)(0, &mut device) == NVML_SUCCESS
    })
}

/// One GPU's counters. Every field is optional for the reason the whole `gpu` service is: a card that does not
/// report a number must read as unknown, never as zero — a hard `0` draws an idle GPU under full load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub name: Option<String>,
    pub usage: Option<u32>,
    pub temperature: Option<u32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

/// Reads GPU `index`, or `None` when NVML is absent or has no such device.
///
/// Cheap enough to call on a poll tick, which is the whole point: this replaces a `fork`/`exec` of
/// `nvidia-smi` and a CSV parse with five function calls into a library already mapped.
pub fn read(index: u32) -> Option<Reading> {
    let nvml = nvml()?;
    unsafe {
        let mut device: Device = std::ptr::null_mut();
        if (nvml.device_by_index)(index, &mut device) != NVML_SUCCESS {
            return None;
        }
        let mut name_buffer = [0 as c_char; NAME_LEN];
        let name = ((nvml.name)(device, name_buffer.as_mut_ptr(), NAME_LEN as c_uint)
            == NVML_SUCCESS)
            .then(|| {
                CStr::from_ptr(name_buffer.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| !name.trim().is_empty());

        let mut utilization = Utilization::default();
        let usage = ((nvml.utilization)(device, &mut utilization) == NVML_SUCCESS)
            .then_some(utilization.gpu);

        let mut celsius: c_uint = 0;
        let temperature = ((nvml.temperature)(device, TEMPERATURE_GPU, &mut celsius)
            == NVML_SUCCESS)
            .then_some(celsius);

        let mut memory = Memory::default();
        let (vram_used, vram_total) = if (nvml.memory)(device, &mut memory) == NVML_SUCCESS {
            (Some(memory.used), Some(memory.total))
        } else {
            (None, None)
        };

        Some(Reading {
            name,
            usage,
            temperature,
            vram_used,
            vram_total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loading is separate from using, so a machine with no NVIDIA driver answers rather than failing to
    /// build or panicking — the same shape the PAM loader is tested in.
    #[test]
    fn asking_on_a_machine_without_nvidia_answers_rather_than_panicking() {
        // Whichever way this machine goes, both branches must be reachable without a crash.
        let _ = available();
        let reading = read(0);
        if let Some(reading) = reading {
            // A card that answers at all must give its name; the counters are each allowed to be absent.
            assert!(
                reading.usage.is_some() || reading.temperature.is_some() || reading.name.is_some(),
                "a device that resolved reported nothing at all"
            );
        }
    }

    /// A device index no machine has must not be reported as a GPU reading zero.
    #[test]
    fn an_index_that_does_not_exist_is_none_rather_than_empty() {
        assert_eq!(read(u32::MAX), None);
    }
}
