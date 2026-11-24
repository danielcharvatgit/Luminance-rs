//! Tiny EDID parser: just enough to get a human readable monitor name.

pub fn monitor_name(edid: &[u8]) -> Option<String> {
    if edid.len() < 128 || edid[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }

    // Four 18-byte descriptors start at offset 54; 0xFC is "monitor name".
    let name = (0..4).find_map(|i| {
        let d = &edid[54 + i * 18..54 + (i + 1) * 18];
        (d[0] == 0 && d[1] == 0 && d[3] == 0xFC).then(|| {
            let text: String = d[5..]
                .iter()
                .take_while(|&&b| b != 0x0A && b != 0)
                .map(|&b| b as char)
                .collect();
            text.trim().to_string()
        })
    });

    let manufacturer = manufacturer_id(edid);
    match name.filter(|n| !n.is_empty()) {
        Some(name) if !manufacturer.is_empty() && !name.to_uppercase().starts_with(&manufacturer) => {
            Some(format!("{manufacturer} {name}"))
        }
        Some(name) => Some(name),
        None if !manufacturer.is_empty() => {
            let product = u16::from_le_bytes([edid[10], edid[11]]);
            Some(format!("{manufacturer} {product:04X}"))
        }
        None => None,
    }
}

fn manufacturer_id(edid: &[u8]) -> String {
    let raw = u16::from_be_bytes([edid[8], edid[9]]);
    [(raw >> 10) & 0x1F, (raw >> 5) & 0x1F, raw & 0x1F]
        .iter()
        .filter(|&&c| (1..=26).contains(&c))
        .map(|&c| (b'@' + c as u8) as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_descriptor() {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // "DEL"
        edid[8] = 0x10;
        edid[9] = 0xAC;
        let d = 54;
        edid[d + 3] = 0xFC;
        edid[d + 5..d + 18].copy_from_slice(b"U2720Q\n      ");
        assert_eq!(monitor_name(&edid).as_deref(), Some("DEL U2720Q"));
    }
}
