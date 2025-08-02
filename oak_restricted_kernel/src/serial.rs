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

use oak_sev_guest::{io::{PortFactoryWrapper, PortWrapper}, msr::SevStatus};
use spinning_top::Spinlock;

// Simple wrapper that looks like uart_16550::SerialPort but uses GHCB when needed
struct SerialPortWrapper {
    direct_port: Option<uart_16550::SerialPort>,
    ghcb_port: Option<sev_serial::SerialPort<PortFactoryWrapper, PortWrapper<u8>, PortWrapper<u8>>>,
}

impl SerialPortWrapper {
    unsafe fn new(base: u16, sev_status: SevStatus) -> Self {
        if sev_status.contains(SevStatus::SEV_ES_ENABLED) {
            let port_factory = crate::ghcb::get_ghcb_port_factory();
            let mut ghcb_port = sev_serial::SerialPort::new(base, port_factory);
            ghcb_port.init().expect("couldn't initialize GHCB serial port");
            Self { direct_port: None, ghcb_port: Some(ghcb_port) }
        } else {
            let mut direct_port = uart_16550::SerialPort::new(base);
            direct_port.init();
            Self { direct_port: Some(direct_port), ghcb_port: None }
        }
    }
    
    fn send(&mut self, data: u8) {
        if let Some(ref mut port) = self.direct_port {
            port.send(data);
        } else if let Some(ref mut port) = self.ghcb_port {
            port.send(data).expect("failed to send via GHCB");
        }
    }
    
    fn receive(&mut self) -> u8 {
        if let Some(ref mut port) = self.direct_port {
            port.receive()
        } else if let Some(ref mut port) = self.ghcb_port {
            port.receive()
        } else {
            0
        }
    }
}

pub struct Serial {
    port: Spinlock<SerialPortWrapper>,
}

// Base I/O port for the second serial port in the system (colloquially known as
// COM2)
static COM2_BASE: u16 = 0x2f8;

impl Serial {
    pub fn new(sev_status: SevStatus) -> Serial {
        let port = unsafe { SerialPortWrapper::new(COM2_BASE, sev_status) };
        Serial { port: Spinlock::new(port) }
    }
}

impl oak_channel::Write for Serial {
    fn write_all(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let mut port = self.port.lock();
        for byte in data {
            port.send(*byte);
        }
        Ok(())
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl oak_channel::Read for Serial {
    fn read_exact(&mut self, data: &mut [u8]) -> anyhow::Result<()> {
        let mut port = self.port.lock();
        for i in 0..data.len() {
            data[i] = port.receive();
        }
        Ok(())
    }
}
