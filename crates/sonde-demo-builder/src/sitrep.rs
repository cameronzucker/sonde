//! Builds the SITREP envelope payload and its field-offset map.

use serde::Serialize;

/// One labeled byte range within the payload.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

/// Field-offset map serialized to `payload.offsets.json`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldOffsets {
    pub total_len: usize,
    pub fields: Vec<Field>,
    pub image_byte_len: usize,
}

/// Build the payload bytes and offsets from envelope text parts and the
/// compressed image bytes. Layout: header + blank line + body + attachment
/// marker line + raw image bytes.
pub fn build_payload(
    callsign: &str,
    position_line: &str,
    body: &str,
    image_jpeg: &[u8],
) -> (Vec<u8>, FieldOffsets) {
    let header = format!(
        "To: EMCOMM-NET\nFrom: {callsign}\nSubject: SITREP - Disaster Area Recon\nDate: 2026-06-14 18:30Z\n{position_line}\n"
    );
    let body_block = format!("\n{body}\n");
    let marker = format!(
        "\n--- attachment: recon.jpg ({} bytes) ---\n",
        image_jpeg.len()
    );

    let mut bytes = Vec::new();
    let header_start = 0;
    bytes.extend_from_slice(header.as_bytes());
    let header_end = bytes.len();

    bytes.extend_from_slice(body_block.as_bytes());
    bytes.extend_from_slice(marker.as_bytes());
    let body_end = bytes.len();

    bytes.extend_from_slice(image_jpeg);
    let image_end = bytes.len();

    let offsets = FieldOffsets {
        total_len: bytes.len(),
        fields: vec![
            Field {
                label: "header".into(),
                start: header_start,
                end: header_end,
            },
            Field {
                label: "body".into(),
                start: header_end,
                end: body_end,
            },
            Field {
                label: "image".into(),
                start: body_end,
                end: image_end,
            },
        ],
        image_byte_len: image_jpeg.len(),
    };
    (bytes, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_partition_the_payload_contiguously() {
        let img = vec![0xABu8; 100];
        let (bytes, off) = build_payload(
            "KK6XYZ",
            "Position: 34-12.34N / 118-29.10W (DM04xf)",
            "Levee breach.",
            &img,
        );
        assert_eq!(off.total_len, bytes.len());
        assert_eq!(off.image_byte_len, 100);
        // Fields are contiguous and cover the whole payload.
        assert_eq!(off.fields[0].start, 0);
        assert_eq!(off.fields[0].end, off.fields[1].start);
        assert_eq!(off.fields[1].end, off.fields[2].start);
        assert_eq!(off.fields.last().unwrap().end, bytes.len());
        // Image region equals the appended image bytes.
        let img_field = &off.fields[2];
        assert_eq!(&bytes[img_field.start..img_field.end], img.as_slice());
    }
}
