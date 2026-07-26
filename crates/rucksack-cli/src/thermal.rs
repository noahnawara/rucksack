//! Thermal pressure, read from the source that actually answers on Apple silicon.
//!
//! `pmset -g therm` reports `CPU_Speed_Limit` and `CPU_Scheduler_Limit`, Intel-era counters that
//! Apple silicon never populates. On the only hardware rucksack targets it prints three
//! "has been recorded" notes and nothing else, at any temperature, so on its own it can never
//! report heat and the thermal release condition could never fire.
//!
//! `ProcessInfo.thermalState` is the public, documented signal for the same question. It needs no
//! privilege, which is why it is read here in the unprivileged watcher rather than in the root
//! helper: the privileged process has no reason to link Foundation or to sample temperature.

use rucksack_core::power::ThermalLevel;

/// The thermal levels that make a closed lid unsafe.
///
/// `Fair` is deliberately not one of them. It is ordinary for a Mac doing the work rucksack exists
/// to protect, and releasing on it would end sessions that are behaving exactly as intended.
pub fn ends_a_lease(level: ThermalLevel) -> bool {
    matches!(level, ThermalLevel::Serious | ThermalLevel::Critical)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::{c_char, c_void};
    use std::mem;

    /// `NSProcessInfoThermalState`, as documented by Apple.
    const NOMINAL: isize = 0;
    const FAIR: isize = 1;
    const SERIOUS: isize = 2;
    const CRITICAL: isize = 3;

    type Object = *mut c_void;
    type Selector = *mut c_void;
    type SendObject = extern "C" fn(Object, Selector) -> Object;
    type SendInteger = extern "C" fn(Object, Selector) -> isize;

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Object;
        fn sel_registerName(name: *const c_char) -> Selector;
        fn objc_msgSend();
    }

    /// Map an `NSProcessInfoThermalState` onto rucksack's level.
    ///
    /// An unrecognised value is `None` rather than a guess, so a level added by a later macOS is
    /// treated as an unread sensor instead of being folded into one of today's.
    fn level_for(state: isize) -> Option<ThermalLevel> {
        match state {
            NOMINAL => Some(ThermalLevel::Nominal),
            FAIR => Some(ThermalLevel::Fair),
            SERIOUS => Some(ThermalLevel::Serious),
            CRITICAL => Some(ThermalLevel::Critical),
            _ => None,
        }
    }

    /// Ask macOS how hot this Mac is.
    ///
    /// `None` means macOS did not answer. That is silence rather than heat, and no caller may
    /// treat it as a reason to end a lease.
    pub fn read_thermal_state() -> Option<ThermalLevel> {
        // SAFETY: `objc_msgSend` has no single C signature — the ABI requires calling it through a
        // pointer typed exactly like the method being sent, which is why each send is transmuted
        // rather than declared. `processInfo` is a class method returning `id`, matching
        // `SendObject`; `thermalState` returns `NSInteger`, which is `isize` on both architectures
        // this ships for. Getting either wrong would corrupt the call, so the two are kept
        // separate rather than shared.
        //
        // `processInfo` returns a shared singleton, not an owned or autoreleased object, and the
        // only value read out is a scalar, so there is nothing to release and no autorelease pool
        // is needed: a million calls moved RSS by 64 KB, which is page noise rather than a leak.
        // No pointer is dereferenced here and none is retained past the call.
        //
        // `thermalState` has existed since macOS 10.10.3 and release packaging targets macOS 14,
        // so the selector is always present. The null checks cover the runtime failing to hand
        // back the class or its shared instance at all, which is the only way this can go wrong.
        unsafe {
            let class = objc_getClass(c"NSProcessInfo".as_ptr());
            if class.is_null() {
                return None;
            }
            let send_object = mem::transmute::<*const (), SendObject>(objc_msgSend as *const ());
            let info = send_object(class, sel_registerName(c"processInfo".as_ptr()));
            if info.is_null() {
                return None;
            }
            let send_integer = mem::transmute::<*const (), SendInteger>(objc_msgSend as *const ());
            level_for(send_integer(
                info,
                sel_registerName(c"thermalState".as_ptr()),
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_every_documented_thermal_state() {
            assert_eq!(level_for(NOMINAL), Some(ThermalLevel::Nominal));
            assert_eq!(level_for(FAIR), Some(ThermalLevel::Fair));
            assert_eq!(level_for(SERIOUS), Some(ThermalLevel::Serious));
            assert_eq!(level_for(CRITICAL), Some(ThermalLevel::Critical));
        }

        #[test]
        fn an_unrecognised_thermal_state_is_not_a_guess() {
            assert_eq!(level_for(4), None);
            assert_eq!(level_for(-1), None);
        }

        /// The whole point of the FFI is that this Mac answers where `pmset -g therm` does not.
        #[test]
        fn this_mac_reports_a_thermal_state() {
            assert!(read_thermal_state().is_some());
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::read_thermal_state;

/// Ask macOS how hot this Mac is.
///
/// `None` means macOS did not answer. That is silence rather than heat, and no caller may treat it
/// as a reason to end a lease.
#[cfg(not(target_os = "macos"))]
pub fn read_thermal_state() -> Option<ThermalLevel> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_serious_and_critical_end_a_lease() {
        assert!(ends_a_lease(ThermalLevel::Serious));
        assert!(ends_a_lease(ThermalLevel::Critical));
        assert!(!ends_a_lease(ThermalLevel::Nominal));
        assert!(!ends_a_lease(ThermalLevel::Fair));
        assert!(!ends_a_lease(ThermalLevel::Unknown));
    }
}
