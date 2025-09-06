//
// Copyright 2025 The Project Oak Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

use alloc::{boxed::Box, rc::Rc};
use core::{ffi::CStr, fmt::Display, ops::Range};

use spinning_top::Spinlock;
use zerocopy::{FromBytes, FromZeros, IntoBytes};

use crate::{
    fw_cfg::Firmware,
    pci::{
        config_access::{ConfigAccess, CAM},
        device::PciBar,
    },
    Platform, ZeroPage,
};

mod config_access;
mod device;
mod machine;
mod resource_allocator;

use device::Bdf;
use machine::{I440fx, Machine, Q35};
use resource_allocator::ResourceAllocator;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes)]
pub struct PciCrsAllowlistEntry {
    pub address: u32,
    pub length: u32,
}
static_assertions::assert_eq_size!(PciCrsAllowlistEntry, [u8; 8]);

pub const PCI_CRS_ALLOWLIST_MAX_ENTRY_COUNT: usize = 11;

const PCI_CRS_ALLOWLIST_FILE_NAME: &CStr = c"etc/pci-crs-whitelist";
const EXTRA_ROOTS_FILE_NAME: &CStr = c"etc/extra-pci-roots";

/// PCI class codes.
///
/// We use a struct instead of enum because Rust enums are closed, but we will
/// not give an exhaustive list of class codes.
///
/// See the PCI Code and ID Assignment Specification on pcisig.com for the
/// authoritative source.
#[derive(PartialEq, Eq)]
#[repr(transparent)]
struct PciClass(pub u8);

impl PciClass {
    pub const BRIDGE: PciClass = PciClass(0x06);
}

impl Display for PciClass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}", self.0)
    }
}
/// Subclass types for PCI devices
///
/// Again, we only list a subset, so we use a struct instead of an enum.
///
/// See the PCI Code and ID Assignment Specification on pcisig.com for the
/// authoritative source.
#[derive(PartialEq, Eq)]
#[repr(transparent)]
struct PciSubclass(pub u8);
impl PciSubclass {
    #[allow(dead_code)]
    pub const HOST_BRIDGE: PciSubclass = PciSubclass(0x00);
    pub const PCI_TO_PCI_BRIDGE: PciSubclass = PciSubclass(0x04);
}

impl Display for PciSubclass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}", self.0)
    }
}

#[derive(Debug, FromBytes, IntoBytes)]
#[repr(C, packed)]
#[allow(dead_code)]
struct PciBridgeBusRegister {
    pub secondary_latency_timer: u8,
    pub subordinate_bus_number: u8,
    pub secondary_bus_number: u8,
    pub primary_bus_number: u8,
}

struct BarIter {
    device: Bdf,
    // Bridges have up to 2 BARs, normal devices 6.
    max_bars: u8,
    index: Option<u8>,
    access: Rc<Spinlock<Box<dyn ConfigAccess>>>,
}

impl Iterator for BarIter {
    type Item = PciBar;

    fn next(&mut self) -> Option<Self::Item> {
        // Try to find a next BAR.
        loop {
            self.index = self.index.filter(|&index| index < self.max_bars);
            let index = self.index?;

            let bar = PciBar::new(self.device, index, self.access.lock().as_mut()).ok()?;
            // We've consumed at least one entry.
            let _ = self.index.insert(index + 1);
            match bar {
                None => {
                    // Unimplemented BAR.
                    continue;
                }
                Some(PciBar::Io { .. }) | Some(PciBar::Memory32 { .. }) => {
                    return bar;
                }
                Some(PciBar::Memory64 { .. }) => {
                    // 64-bit memory BARs take two slots.
                    let _ = self.index.insert(index + 2);
                    return bar;
                }
            }
        }
    }
}

/// PCI address.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
struct PciAddress(Bdf);

impl PciAddress {
    pub fn new(bus: u8, device: u8, function: u8) -> Result<Self, &'static str> {
        Bdf::new(bus, device, function).map(Self)
    }

    /// Returns the Vendor ID and Device ID for the address.
    fn vendor_device_id(&self, access: &mut dyn ConfigAccess) -> Result<(u16, u16), &'static str> {
        // Register 0x00: Device ID, Vendor ID (16b each)
        let value = access.read(self.0, 0x00)?;
        Ok(((value & 0xFFFF) as u16, (value >> 16) as u16))
    }

    fn class_code(
        &self,
        access: &mut dyn ConfigAccess,
    ) -> Result<(PciClass, PciSubclass), &'static str> {
        // Register 0x02: Class Code, Subclass, Prog IF, Revision ID (8b each)
        let value = access.read(self.0, 0x02)?;
        Ok((PciClass((value >> 24) as u8), PciSubclass((value >> 16) as u8)))
    }

    // Returns the header type for the address.
    fn header_type(&self, access: &mut dyn ConfigAccess) -> Result<u8, &'static str> {
        // Register 0x03: BIST, header type, latency timer, cache line size (8b each)
        let value = access.read(self.0, 0x03)?;
        Ok((value >> 16) as u8)
    }

    fn bridge_bus_numbers(
        &self,
        access: &mut dyn ConfigAccess,
    ) -> Result<PciBridgeBusRegister, &'static str> {
        let value = access.read(self.0, 0x06)?;
        Ok(PciBridgeBusRegister::read_from_bytes(value.as_bytes()).unwrap())
    }

    fn is_multi_function_device(
        &self,
        access: &mut dyn ConfigAccess,
    ) -> Result<bool, &'static str> {
        self.header_type(access).map(|value| value & 0x80 != 0)
    }

    fn iter_bars(
        &self,
        access: Rc<Spinlock<Box<dyn ConfigAccess>>>,
    ) -> Result<BarIter, &'static str> {
        let (class, subclass) = self.class_code(access.lock().as_mut())?;
        let max_bars = if class == PciClass::BRIDGE && subclass == PciSubclass::PCI_TO_PCI_BRIDGE {
            2
        } else {
            6
        };
        Ok(BarIter { device: self.0, max_bars, index: Some(0), access })
    }
    /// Checks if the device exists at all.
    fn exists(&self, access: &mut dyn ConfigAccess) -> Result<bool, &'static str> {
        let (vendor_id, _) = self.vendor_device_id(access)?;
        Ok(vendor_id != 0xFFFF && vendor_id != 0x0000)
    }

    /// Returns the first function on the next device on the bus.
    ///
    /// Returns None if this is the last device on this device.
    pub fn next_device(&self) -> Option<Self> {
        self.0.next_device().map(Self)
    }

    /// Returns the next function on the bus, crossing to the next device if
    /// needed.
    ///
    /// Returns None if this is the last function on this bus.
    pub fn next(&self) -> Option<Self> {
        self.0.next().map(Self)
    }
}

impl Display for PciAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<(u8, u8, u8)> for PciAddress {
    type Error = &'static str;

    fn try_from((bus, device, function): (u8, u8, u8)) -> Result<Self, Self::Error> {
        PciAddress::new(bus, device, function)
    }
}

impl From<PciAddress> for u16 {
    fn from(value: PciAddress) -> Self {
        value.0.into()
    }
}

struct BusDeviceIterator {
    address: Option<PciAddress>,
    access: Rc<Spinlock<Box<dyn ConfigAccess>>>,
}

impl Iterator for BusDeviceIterator {
    type Item = PciAddress;

    fn next(&mut self) -> Option<Self::Item> {
        // Shortcircuit if we've exhausted the bus.
        let current_address = self.address?;

        // Shortcut: if we know we're on a single-function device, step over to the next
        // device.
        self.address =
            if current_address.is_multi_function_device(self.access.lock().as_mut()).unwrap() {
                current_address.next()
            } else {
                current_address.next_device()
            };

        // Ensure said next device actually exists.
        while let Some(address) = self.address {
            // Does the device exist?
            if address.exists(self.access.lock().as_mut()).unwrap() {
                // Yes, it does.
                break;
            }
            // No; skip to the next function.
            // For future: can we skip all the functions and skip to the next device if
            // function() == 0? Inside a multi-function device there can be
            // gaps, but we'll need to read from the spec if function 0 is
            // always assumed to exist in multi-function devices.
            self.address = address.next();
        }

        Some(current_address)
    }
}

struct PciBus {
    pub root: PciAddress,
}

impl PciBus {
    fn new(bus: u8, access: &mut dyn ConfigAccess) -> Result<Option<Self>, &'static str> {
        let root = PciAddress::new(bus, 0, 0)?;
        if root.exists(access)? {
            Ok(Some(Self { root }))
        } else {
            Ok(None)
        }
    }

    fn init(
        &mut self,
        windows: &PciWindows,
        config_access: Rc<Spinlock<Box<dyn ConfigAccess>>>,
    ) -> Result<(), &'static str> {
        // Prepare the allocators for all the resources.
        let mut io_allocator = ResourceAllocator::new(windows.pci_window_16.clone());
        let mut mem32_allocator = ResourceAllocator::new(windows.pci_window_32.clone());
        let mut mem64_allocator = ResourceAllocator::new(windows.pci_window_64.clone());

        for function in self.iter_devices(config_access.clone()) {
            let (vendor_id, device_id) =
                function.vendor_device_id(config_access.lock().as_mut())?;
            let (class, subclass) = function.class_code(config_access.lock().as_mut())?;

            log::debug!(
                "Found PCI device: {}, {:04x}:{:04x}, class: {}{}",
                function,
                vendor_id,
                device_id,
                class,
                subclass
            );

            if class == PciClass::BRIDGE && subclass == PciSubclass::PCI_TO_PCI_BRIDGE {
                let bridge_bus_numbers =
                    function.bridge_bus_numbers(config_access.lock().as_mut())?;
                log::debug!("PCI to PCI bridge:  {:?}", bridge_bus_numbers);
                log::warn!(
                    "UNIMPLEMENTED: leaving PCI bridge unconfigured, file a bug if you see this!"
                );
            }

            for mut bar in function.iter_bars(config_access.clone())? {
                match bar {
                    PciBar::Memory32 { offset, bar_size, .. } => {
                        log::debug!("  BAR{}: memory, size {}", offset, bar_size);
                        let allocation = mem32_allocator
                            .allocate(bar_size)
                            .ok_or("out of memory for 32-bit memory BAR")?
                            .start;
                        log::debug!(
                            "    assigning [0x{:08x}-0x{:08x})",
                            allocation,
                            allocation + bar_size
                        );
                        bar.set_address(allocation.into(), config_access.lock().as_mut())?;
                    }
                    PciBar::Memory64 { offset, bar_size, .. } => {
                        log::debug!("  BAR{}: memory, 64-bit pref, size {}", offset, bar_size);
                        let allocation = mem64_allocator
                            .allocate(bar_size)
                            .ok_or("out of memory for 64-bit memory BAR")?
                            .start;
                        log::debug!(
                            "    assigning [0x{:016x}-0x{:016x})",
                            allocation,
                            allocation + bar_size
                        );
                        bar.set_address(allocation, config_access.lock().as_mut())?;
                    }
                    PciBar::Io { offset, bar_size, .. } => {
                        log::debug!("  BAR{}: I/O, size {}", offset, bar_size);
                        let bar_size = bar_size.try_into().unwrap();
                        let allocation = io_allocator
                            .allocate(bar_size)
                            .ok_or("out of memory for 64-bit memory BAR")?
                            .start;
                        log::debug!(
                            "    assigning [0x{:04x}-0x{:04x})",
                            allocation,
                            allocation + bar_size
                        );
                        bar.set_address(allocation.into(), config_access.lock().as_mut())?;
                    }
                }
            }
        }
        Ok(())
    }

    fn iter_devices(&self, access: Rc<Spinlock<Box<dyn ConfigAccess>>>) -> BusDeviceIterator {
        BusDeviceIterator { address: Some(self.root), access }
    }
}

/// Location of the PCI resources on this machine.
#[derive(Debug)]
pub struct PciWindows {
    pub pci_window_16: Range<u16>,
    // These are still memory addresses, but we use u32 here as they must be in 32-bit memory.
    pub pci_window_32: Range<u32>,
    pub pci_window_64: Range<u64>,
}

fn init_machine<P: Platform, M: Machine>(
    mut root_bus: PciBus,
    firmware: &mut dyn Firmware,
    zero_page: &mut ZeroPage,
    config_access: Rc<Spinlock<Box<dyn ConfigAccess>>>,
) -> Result<Option<PciWindows>, &'static str> {
    // Determine the PCI holes. How this is done is unfortunately extremely clunky
    // and machine-specific.
    let pci_windows = PciWindows {
        pci_window_16: M::io_port_range(firmware, zero_page)?,
        pci_window_32: M::mmio32_hole(firmware, zero_page)?,
        pci_window_64: M::mmio64_hole::<P>(firmware, zero_page)?,
    };

    log::info!("PCI: using windows {:?}", pci_windows);

    root_bus.init(&pci_windows, config_access)?;

    // Find out if there are any extra roots.
    let extra_roots = read_extra_roots(firmware)?;
    if extra_roots > 0 {
        log::debug!("{} extra root buses reported by VMM", extra_roots);
    }
    Ok(Some(pci_windows))
}

pub fn init<P: Platform>(
    firmware: &mut dyn Firmware,
    zero_page: &mut ZeroPage,
) -> Result<Option<PciWindows>, &'static str> {
    // At this point we know nothing about the platform we're on, so we have to
    // rely on the legacy CAM to get the device ID of the first PCI root to
    // help us figure out on what kind of machine we are running.

    // This type is convoluted because...
    // (a) we need an Rc because of reference counting;
    // (b) we need Spinlock to enforce mutual exclusion (technically not an issue
    // because we don't have concurrency in stage0 but Rust enforces that)
    // (c) a Box because we want `dyn ConfigAccess`, not a concrete type.
    let config_access: Rc<Spinlock<Box<dyn ConfigAccess>>> =
        Rc::new(Spinlock::new(Box::new(CAM::new::<P>())));

    let root_bus = match PciBus::new(0, config_access.lock().as_mut())? {
        Some(bus) => bus,
        None => return Ok(None),
    };

    let root_bridge_device_id =
        root_bus.root.vendor_device_id(config_access.clone().lock().as_mut())?;
    match root_bridge_device_id {
        (I440fx::PCI_VENDOR_ID, I440fx::PCI_DEVICE_ID) => {
            init_machine::<P, I440fx>(root_bus, firmware, zero_page, config_access)
        }
        (Q35::PCI_VENDOR_ID, Q35::PCI_DEVICE_ID) => {
            init_machine::<P, Q35>(root_bus, firmware, zero_page, config_access)
        }
        (vendor_id, device_id) => {
            log::error!(
                "Unknown PCI root device: {:04x}:{:04x} -- will not initialize PCI bus",
                vendor_id,
                device_id
            );
            Ok(None)
        }
    }
}

fn read_extra_roots(firmware: &mut dyn Firmware) -> Result<u64, &'static str> {
    if let Some(file) = firmware.find(EXTRA_ROOTS_FILE_NAME) {
        if file.size() > core::mem::size_of::<u64>() {
            return Ok(0);
        }
        let mut roots: u64 = 0;
        firmware.read_file(&file, roots.as_mut_bytes())?;
        return Ok(roots);
    }

    // File not found, no extra roots.
    Ok(0)
}

pub fn read_pci_crs_allowlist(
    firmware: &mut dyn Firmware,
) -> Result<Option<[PciCrsAllowlistEntry; PCI_CRS_ALLOWLIST_MAX_ENTRY_COUNT]>, &'static str> {
    let file = match firmware.find(PCI_CRS_ALLOWLIST_FILE_NAME) {
        Some(file) => file,
        None => return Ok(None),
    };
    if file.size() % size_of::<PciCrsAllowlistEntry>() != 0 {
        return Err("invalid etc/pci-crs-whitelist file size");
    }
    if file.size() > PCI_CRS_ALLOWLIST_MAX_ENTRY_COUNT * size_of::<PciCrsAllowlistEntry>() {
        return Err("too many entries in etc/pci-crs-whitelist");
    }
    let mut entries = [PciCrsAllowlistEntry::new_zeroed(); PCI_CRS_ALLOWLIST_MAX_ENTRY_COUNT];
    firmware.read_file(&file, &mut entries.as_mut_bytes()[..file.size()])?;

    Ok(Some(entries))
}

#[cfg(test)]
mod tests {
    use googletest::prelude::*;

    use super::*;
    use crate::fw_cfg::TestFirmware;

    #[googletest::test]
    fn test_allowlist() {
        let mut firmware = TestFirmware::default();
        firmware
            .files
            .insert(PCI_CRS_ALLOWLIST_FILE_NAME.to_owned(), Box::new([1, 2, 3, 4, 5, 6, 7, 8]));

        let result = read_pci_crs_allowlist(&mut firmware);

        assert_that!(
            result,
            ok(some(all!(
                contains(eq(PciCrsAllowlistEntry { address: 0x04030201, length: 0x08070605 })),
                contains(eq(PciCrsAllowlistEntry::new_zeroed()))
            )))
        );
    }

    #[googletest::test]
    fn test_no_allowlist() {
        let mut firmware = TestFirmware::default();

        assert_that!(read_pci_crs_allowlist(&mut firmware), ok(none()));
    }

    #[googletest::test]
    fn test_allowlist_too_large() {
        let mut firmware = TestFirmware::default();
        firmware.files.insert(
            PCI_CRS_ALLOWLIST_FILE_NAME.to_owned(),
            Box::new([0; (PCI_CRS_ALLOWLIST_MAX_ENTRY_COUNT + 1) * 8]),
        );

        assert_that!(read_pci_crs_allowlist(&mut firmware), err(anything()));
    }

    #[googletest::test]
    fn test_allowlist_garbage() {
        let mut firmware = TestFirmware::default();
        firmware.files.insert(
            PCI_CRS_ALLOWLIST_FILE_NAME.to_owned(),
            Box::new([0; size_of::<PciCrsAllowlistEntry>() + 1]),
        );

        assert_that!(read_pci_crs_allowlist(&mut firmware), err(anything()));
    }
}
