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

mod page;
mod stage0;
mod vmsa;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use log::trace;
use page::PageInfo;

use x86_64::structures::paging::{PageSize, Size4KiB};

use crate::{
    stage0::{load_stage0, SnpRomParsing},
    vmsa::{get_ap_vmsa, get_boot_vmsa, VMSA_ADDRESS, VMSA_ADDRESS_QEMU},
};

#[derive(Parser, Clone)]
#[command(about = "Oak SEV-SNP Measurement Calculator")]
struct Cli {
    #[arg(long, help = "The location of the Stage 0 firmware ROM image")]
    stage0_rom: Option<PathBuf>,
    #[arg(long, help = "Whether the firwmare is shadowed to support legacy boot")]
    legacy_boot: bool,
    #[arg(long, help = "The number of vCPUs available to the VM at boot", default_values_t = [1], value_delimiter = ',', num_args = 1..)]
    vcpu_count: Vec<usize>,
    #[arg(
        long,
        help = "The dir to output the predicted attestation measurements as binary files to"
    )]
    attestation_measurements_output_dir: Option<PathBuf>,
    #[arg(
        long,
        help = "CPU family for VMSA signature calculation",
        default_value = "6"
    )]
    cpu_family: u8,
    #[arg(
        long,
        help = "CPU model for VMSA signature calculation",
        default_value = "0"
    )]
    cpu_model: u8,
    #[arg(
        long,
        help = "CPU stepping for VMSA signature calculation",
        default_value = "0"
    )]
    cpu_stepping: u8,
    #[arg(long, help = "Use QEMU-compatible VMSA settings")]
    qemu: bool,
}

impl Cli {
    fn stage0_path(&self) -> PathBuf {
        self.stage0_rom
            .clone()
            .expect("need to specify --stage0_rom")
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let stage0 = load_stage0(cli.stage0_path())?;

    let mut base_page_info = PageInfo::new();

    // Add the Stage 0 firmware ROM image.
    base_page_info.update_from_data(stage0.rom_bytes(), stage0.start_address);

    if cli.legacy_boot {
        // Add the legacy boot shadow of the Stage 0 firmware ROM image.
        base_page_info.update_from_data(stage0.legacy_shadow_bytes(), stage0.legacy_start_address);
    }

    for snp_page in stage0.get_snp_pages(cli.qemu) {
        for page_number in 0..snp_page.page_count {
            base_page_info.update_from_snp_page(
                snp_page.page_type.clone(),
                snp_page.start_address + (page_number as u64) * Size4KiB::SIZE,
            );
        }
    }

    let vmsa_address = if cli.qemu {
        VMSA_ADDRESS_QEMU
    } else {
        VMSA_ADDRESS
    };

    // The boot vCPU has the default VMSA configured.
    base_page_info.update_from_vmsa(
        &get_boot_vmsa(cli.cpu_family, cli.cpu_model, cli.cpu_stepping, cli.qemu),
        vmsa_address,
    );

    // Subsequent vCPUs use the IP and CS segment specified in the SEV-ES reset
    // block table in the firmware.
    let sev_es_reset_block = stage0.get_sev_es_reset_block();
    let ap_vmsa = get_ap_vmsa(
        &sev_es_reset_block,
        cli.cpu_family,
        cli.cpu_model,
        cli.cpu_stepping,
        cli.qemu,
    );
    // Derive measurements for each vCPU counts specified.
    for vcpu_count in cli.vcpu_count {
        let mut page_info = base_page_info.clone();
        // Iterate through all vCPUs up to the specified count.
        for _ in 1..vcpu_count {
            page_info.update_from_vmsa(&ap_vmsa, vmsa_address);
        }

        trace!(
            "raw measurement for {} vCPU: {:?}",
            vcpu_count,
            page_info.digest_cur
        );

        println!(
            "Attestation Measurement {} vCPU: {}",
            vcpu_count,
            hex::encode(page_info.digest_cur)
        );

        if let Some(mut path) = cli.attestation_measurements_output_dir.clone() {
            path.push(format!(
                "sha2_384_measurement_of_initial_memory_with_stage0_and_{:02}_vcpu",
                vcpu_count
            ));
            std::fs::write(path, page_info.digest_cur)
                .context("couldn't write attestation measurement")?;
        }
    }

    Ok(())
}
