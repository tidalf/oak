//
// Copyright 2022 The Project Oak Authors
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

use log::trace;
use oak_sev_guest::vmsa::{calculate_rdx_from_fms, Vmsa, VmsaPage};
use x86_64::{
    structures::paging::{PageSize, Size4KiB},
    PhysAddr,
};

use crate::stage0::SevEsResetBlock;

/// The guest-physical address of the VMSA page.
///
/// The current implementation uses the same fixed address for all VMSA pages.
/// It is calculated as the start-address of the last 4KiB page that can be
/// addressed within the allowed physical bits.
///
/// For AMD "Milan" CPUs the maximum supported physical memory bit-width is 48
/// when SEV-SNP is enabled.
#[allow(unused)]
pub const VMSA_ADDRESS: PhysAddr = PhysAddr::new((1 << 48) - Size4KiB::SIZE);

/// The guest-physical address of the VMSA page (QEMU-compatible).
#[allow(unused)]
pub const VMSA_ADDRESS_QEMU: PhysAddr = PhysAddr::new(0xfffffffff000);

/// Gets the initial VMSA for the vCPU that is used to boot the VM with optional QEMU compatibility.
pub fn get_boot_vmsa(family: u8, model: u8, stepping: u8, qemu_compat: bool) -> VmsaPage {
    let rdx_value = calculate_rdx_from_fms(family, model, stepping);
    let mut result = VmsaPage::new(Vmsa::new_vcpu_boot(rdx_value));
    // We expect a slightly different initial state to use for the measurement.
    if qemu_compat {
        result.vmsa.mxcsr = 0x1f80;
        result.vmsa.x87_fcw = 0x37f;
    } else {
        result.vmsa.g_pat = 0x00070106;
    }

    result.vmsa.sev_features = 0x00000001;

    trace!("Boot VMSA: {:?}", result);
    result
}

/// Gets the initial VMSA for additional vCPUs that are not the boot vCPU with optional QEMU compatibility.
pub fn get_ap_vmsa(
    reset_block: &SevEsResetBlock,
    family: u8,
    model: u8,
    stepping: u8,
    qemu_compat: bool,
) -> VmsaPage {
    let mut result = get_boot_vmsa(family, model, stepping, qemu_compat);
    result.vmsa.rip = reset_block.rip;
    result.vmsa.cs.base = reset_block.segment_base;
    trace!("AP VMSA: {:?}", result);
    result
}
