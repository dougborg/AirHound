//! Explicit USB attachment selection; enumeration never opens a device handle.
use anyhow::{ensure, Context, Result};
use rusb::{Context as UsbContext, Device, UsbContext as _};
use serde::Serialize;
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub bus: u8,
    pub ports: Vec<u8>,
}
impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "usb:{}:{}",
            self.bus,
            self.ports
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(".")
        )
    }
}
impl FromStr for Selector {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parts = text.split(':');
        let invalid = || {
            "USB selector must be canonical usb:<bus>:<port>[.<port>...] (bus 0..255, ports 1..255, at most 7 ports)".to_string()
        };
        if parts.next() != Some("usb") {
            return Err(invalid());
        }
        let bus = parts
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .ok_or_else(invalid)?;
        let ports = parts
            .next()
            .ok_or_else(invalid)?
            .split('.')
            .map(|s| s.parse::<u8>().ok().filter(|n| *n > 0).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.next().is_some() || ports.is_empty() || ports.len() > 7 {
            return Err(invalid());
        }
        let selector = Self { bus, ports };
        if selector.to_string() != text {
            return Err(invalid());
        }
        Ok(selector)
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Identity {
    pub selector: String,
    pub bus: u8,
    pub ports: Vec<u8>,
    /// Diagnostic only: address changes on reconnect and is never a selector.
    pub address: u8,
    pub identity_scope: &'static str,
}
impl Identity {
    fn from_device(device: &Device<UsbContext>) -> Result<Self> {
        let selector = Selector {
            bus: device.bus_number(),
            ports: device.port_numbers().context("reading USB port topology")?,
        };
        let selector: Selector = selector.to_string().parse().map_err(anyhow::Error::msg)?;
        Ok(Self {
            selector: selector.to_string(),
            bus: selector.bus,
            ports: selector.ports,
            address: device.address(),
            identity_scope: "usb_attachment_topology",
        })
    }
}
type Enumerated = (Vec<(Device<UsbContext>, Identity)>, Vec<String>);
pub(crate) fn enumerate() -> Result<Enumerated> {
    let context = UsbContext::new()?;
    let mut found = Vec::new();
    let mut diagnostics = Vec::new();
    for device in context.devices()?.iter() {
        let descriptor = match device.device_descriptor() {
            Ok(descriptor) => descriptor,
            Err(error) => {
                if diagnostics.len() < 16 {
                    diagnostics.push(format!(
                        "USB descriptor unavailable at bus {} address {}: {error}",
                        device.bus_number(),
                        device.address()
                    ));
                } else if diagnostics.len() == 16 {
                    diagnostics.push("Additional USB descriptor errors omitted".into());
                }
                continue;
            }
        };
        if descriptor.vendor_id() == 0x0e8d && descriptor.product_id() == 0x7610 {
            let identity = Identity::from_device(&device)?;
            found.push((device, identity));
        }
    }
    found.sort_by(|a, b| a.1.selector.cmp(&b.1.selector));
    Ok((found, diagnostics))
}
#[derive(Serialize)]
pub struct Listing {
    pub schema_version: u8,
    pub devices: Vec<Identity>,
    pub diagnostics: Vec<String>,
}
pub fn list() -> Result<Listing> {
    let (devices, diagnostics) = enumerate()?;
    Ok(Listing {
        schema_version: 1,
        devices: devices.into_iter().map(|(_, id)| id).collect(),
        diagnostics,
    })
}

/// Pin implicit selection after first successful enumeration, before claiming USB.
/// Reconnect never falls back to another port. Replacing hardware at the same
/// port is not distinguishable without a separately authenticated device identity.
#[derive(Default)]
pub struct Selection {
    selector: Option<Selector>,
}
impl Selection {
    pub fn new(selector: Option<Selector>) -> Self {
        Self { selector }
    }
    pub fn select(&mut self, devices: &[Identity]) -> Result<usize> {
        let matches: Vec<_> = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                self.selector
                    .as_ref()
                    .is_none_or(|s| s.to_string() == d.selector)
            })
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() {
            return Err(
                anyhow::Error::new(rusb::Error::NoDevice).context(match &self.selector {
                    Some(selector) => format!(
                        "selected USB attachment {selector} is absent; no fallback attempted"
                    ),
                    None => "no matching USB0e8d:7610 device".into(),
                }),
            );
        }
        ensure!(matches.len()==1,"Expected exactly one matching USB 0e8d:7610, found {}; use list-devices and --usb-selector",matches.len());
        let i = matches[0];
        self.selector = Some(devices[i].selector.parse().map_err(anyhow::Error::msg)?);
        Ok(i)
    }
    fn select_enumerated(&mut self, devices: &[Identity], diagnostics: &[String]) -> Result<usize> {
        ensure!(self.selector.is_some() || diagnostics.is_empty(), "Cannot prove a unique implicit adapter after USB descriptor errors; use an explicit selector");
        self.select(devices)
    }
    pub fn open(&mut self) -> Result<crate::transport::UsbDevice> {
        let (mut devices, diagnostics) = enumerate()?;
        for message in &diagnostics {
            eprintln!("USB enumeration: {message}");
        }
        let i = self.select_enumerated(
            &devices.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
            &diagnostics,
        )?;
        let (device, identity) = devices.swap_remove(i);
        crate::transport::UsbDevice::open_device(device, identity)
    }
}
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    fn id(selector: &str, address: u8) -> Identity {
        let s: Selector = selector.parse().unwrap();
        Identity {
            selector: selector.into(),
            bus: s.bus,
            ports: s.ports,
            address,
            identity_scope: "usb_attachment_topology",
        }
    }
    #[test]
    fn incomplete_enumeration_requires_explicit_or_previously_pinned_target() {
        let a = id("usb:1:2", 3);
        let warning = vec!["unrelated descriptor unavailable".into()];
        let mut implicit = Selection::default();
        assert!(implicit
            .select_enumerated(std::slice::from_ref(&a), &warning)
            .is_err());
        implicit
            .select_enumerated(std::slice::from_ref(&a), &[])
            .unwrap();
        assert_eq!(
            implicit
                .select_enumerated(std::slice::from_ref(&a), &warning)
                .unwrap(),
            0
        );
        let mut explicit = Selection::new(Some("usb:1:2".parse().unwrap()));
        assert_eq!(explicit.select_enumerated(&[a], &warning).unwrap(), 0);
    }
    #[test]
    fn selectors_are_canonical_and_bounded() {
        for s in ["usb:0:1", "usb:1:2", "usb:255:1.2.3.4.5.6.255"] {
            assert_eq!(s.parse::<Selector>().unwrap().to_string(), s);
        }
        for s in [
            "",
            "1:2",
            "usb:1:0",
            "usb:01:1",
            "usb:1:01",
            "usb:+1:1",
            "usb:256:1",
            "usb:1:1.2.3.4.5.6.7.8",
            "usb:1:1:",
            "usb:1:",
            "usb:1:1 ",
        ] {
            assert!(s.parse::<Selector>().is_err(), "{s}");
        }
    }
    #[test]
    fn default_is_unambiguous_and_then_pinned_across_reconnects() {
        let a = id("usb:1:1.2", 3);
        let b = id("usb:1:1.3", 4);
        let mut select = Selection::default();
        assert!(select.select(&[]).is_err());
        assert!(select.select(&[a.clone(), b.clone()]).is_err());
        assert_eq!(select.select(std::slice::from_ref(&a)).unwrap(), 0);
        assert!(select.select(std::slice::from_ref(&b)).is_err());
        let mut replug = a.clone();
        replug.address = 99;
        assert_eq!(select.select(&[b, replug]).unwrap(), 1);
    }
    #[test]
    fn explicit_selection_never_falls_back_or_accepts_duplicates() {
        let a = id("usb:1:1", 2);
        let b = id("usb:2:1", 3);
        let mut select = Selection::new(Some("usb:2:1".parse().unwrap()));
        assert!(select.select(std::slice::from_ref(&a)).is_err());
        assert_eq!(select.select(&[a, b.clone()]).unwrap(), 1);
        assert!(select.select(&[b.clone(), b]).is_err());
    }
}
