// SPDX-License-Identifier: MPL-2.0
//! Platform-independent BLE transport constants and candidate matching rules.
//! Hosts own scanning, connecting and writing.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BleTransportSpec {
    pub service_uuid: String,
    pub tx_uuid: String,
    pub rx_uuid: String,
    pub write_chunk_size: u16,
    pub ota_write_chunk_limit: u16,
}
#[uniffi::export]
pub fn ble_transport_spec() -> BleTransportSpec {
    BleTransportSpec {
        service_uuid: "6E400001-B5A3-F393-E0A9-E50E24DCCA9E".into(),
        tx_uuid: "6E400003-B5A3-F393-E0A9-E50E24DCCA9E".into(),
        rx_uuid: "6E400002-B5A3-F393-E0A9-E50E24DCCA9E".into(),
        write_chunk_size: 20,
        ota_write_chunk_limit: 244,
    }
}
/// Advertised-name rule: `ring`, `open zilo`, or `open zilo-<non-space>` anywhere.
#[uniffi::export]
pub fn is_compatible_ring_name(name: Option<String>) -> bool {
    let s = name.unwrap_or_default().trim().to_lowercase();
    if s == "ring" || s == "open zilo" {
        return true;
    }
    s.match_indices("open zilo-").any(|(i, m)| {
        s[i + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
    })
}
#[uniffi::export]
pub fn normalize_ble_address(address: String) -> String {
    address
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}
#[uniffi::export]
pub fn ble_address_matches(candidate: String, expected: Option<String>) -> bool {
    let expected = normalize_ble_address(expected.unwrap_or_default());
    expected.is_empty() || normalize_ble_address(candidate) == expected
}
