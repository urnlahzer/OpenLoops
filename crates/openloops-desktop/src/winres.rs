use std::io::{self, Write};

const ICON_ENTRY_SIZE: usize = 16;
const RESOURCE_HEADER_SIZE: u32 = 32;
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

#[derive(Debug)]
pub(crate) struct IconImage<'a> {
    width: u8,
    height: u8,
    color_count: u8,
    planes: u16,
    bit_count: u16,
    data: &'a [u8],
}

pub(crate) fn icon_to_res(icon: &[u8]) -> Result<Vec<u8>, String> {
    let images = parse_icon(icon)?;
    let mut output = Vec::new();
    write_resource_header(&mut output, 0, 0, 0, 0).map_err(|error| error.to_string())?;

    for (index, image) in images.iter().enumerate() {
        let resource_id = u16::try_from(index + 1)
            .map_err(|_| "ICO contains too many images for Win32 resource ids".to_owned())?;
        write_resource(&mut output, RT_ICON, resource_id, image.data)
            .map_err(|error| error.to_string())?;
    }

    let group = make_group_icon(&images)?;
    write_resource(&mut output, RT_GROUP_ICON, 1, &group).map_err(|error| error.to_string())?;
    Ok(output)
}

fn parse_icon(icon: &[u8]) -> Result<Vec<IconImage<'_>>, String> {
    if icon.len() < 6 {
        return Err("ICO is truncated before the ICONDIR header".to_owned());
    }
    let reserved = read_u16(icon, 0);
    if reserved != 0 {
        return Err(format!("ICO reserved field must be 0, found {reserved}"));
    }
    let icon_type = read_u16(icon, 2);
    if icon_type != 1 {
        return Err(format!("ICO type field must be 1, found {icon_type}"));
    }
    let count = usize::from(read_u16(icon, 4));
    if count == 0 {
        return Err("ICO must contain at least one image".to_owned());
    }
    let table_size = count
        .checked_mul(ICON_ENTRY_SIZE)
        .and_then(|size| size.checked_add(6))
        .ok_or_else(|| "ICO directory size overflows".to_owned())?;
    if icon.len() < table_size {
        return Err(format!(
            "ICO is truncated in its directory: expected {table_size} bytes, found {}",
            icon.len()
        ));
    }

    let mut images = Vec::with_capacity(count);
    for index in 0..count {
        let entry = 6 + index * ICON_ENTRY_SIZE;
        let data_size = usize::try_from(read_u32(icon, entry + 8))
            .map_err(|_| format!("ICO image {} size does not fit usize", index + 1))?;
        let data_offset = usize::try_from(read_u32(icon, entry + 12))
            .map_err(|_| format!("ICO image {} offset does not fit usize", index + 1))?;
        let data_end = data_offset
            .checked_add(data_size)
            .ok_or_else(|| format!("ICO image {} range overflows", index + 1))?;
        let data = icon.get(data_offset..data_end).ok_or_else(|| {
            format!(
                "ICO image {} is truncated: byte range {data_offset}..{data_end} exceeds file length {}",
                index + 1,
                icon.len()
            )
        })?;
        images.push(IconImage {
            width: icon[entry],
            height: icon[entry + 1],
            color_count: icon[entry + 2],
            planes: read_u16(icon, entry + 4),
            bit_count: read_u16(icon, entry + 6),
            data,
        });
    }
    Ok(images)
}

fn make_group_icon(images: &[IconImage<'_>]) -> Result<Vec<u8>, String> {
    let count = u16::try_from(images.len())
        .map_err(|_| "ICO contains too many images for a group icon".to_owned())?;
    let mut group = Vec::with_capacity(6 + images.len() * 14);
    write_u16(&mut group, 0).map_err(|error| error.to_string())?;
    write_u16(&mut group, 1).map_err(|error| error.to_string())?;
    write_u16(&mut group, count).map_err(|error| error.to_string())?;
    for (index, image) in images.iter().enumerate() {
        group.extend_from_slice(&[image.width, image.height, image.color_count, 0]);
        write_u16(&mut group, image.planes).map_err(|error| error.to_string())?;
        write_u16(&mut group, image.bit_count).map_err(|error| error.to_string())?;
        write_u32(
            &mut group,
            u32::try_from(image.data.len())
                .map_err(|_| format!("ICO image {} is larger than 4 GiB", index + 1))?,
        )
        .map_err(|error| error.to_string())?;
        write_u16(
            &mut group,
            u16::try_from(index + 1)
                .map_err(|_| "ICO contains too many images for Win32 resource ids".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(group)
}

fn write_resource(
    output: &mut Vec<u8>,
    resource_type: u16,
    id: u16,
    data: &[u8],
) -> io::Result<()> {
    let data_size = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "resource exceeds 4 GiB"))?;
    write_resource_header(output, data_size, resource_type, id, 0x1010)?;
    output.extend_from_slice(data);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    Ok(())
}

fn write_resource_header(
    output: &mut Vec<u8>,
    data_size: u32,
    resource_type: u16,
    id: u16,
    memory_flags: u16,
) -> io::Result<()> {
    write_u32(output, data_size)?;
    write_u32(output, RESOURCE_HEADER_SIZE)?;
    write_u16(output, u16::MAX)?;
    write_u16(output, resource_type)?;
    write_u16(output, u16::MAX)?;
    write_u16(output, id)?;
    write_u32(output, 0)?;
    write_u16(output, memory_flags)?;
    write_u16(output, 0)?;
    write_u32(output, 0)?;
    write_u32(output, 0)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_u16(output: &mut impl Write, value: u16) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn write_u32(output: &mut impl Write, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::{icon_to_res, read_u16, read_u32};

    fn synthetic_icon() -> Vec<u8> {
        let first = [1, 2, 3];
        let second = [4, 5, 6, 7, 8];
        let mut icon = Vec::new();
        icon.extend_from_slice(&[0, 0, 1, 0, 2, 0]);
        add_icon_entry(&mut icon, 16, 16, 3, 38);
        add_icon_entry(&mut icon, 0, 0, 5, 41);
        icon.extend_from_slice(&first);
        icon.extend_from_slice(&second);
        icon
    }

    fn add_icon_entry(icon: &mut Vec<u8>, width: u8, height: u8, size: u32, offset: u32) {
        icon.extend_from_slice(&[width, height, 0, 0]);
        icon.extend_from_slice(&1_u16.to_le_bytes());
        icon.extend_from_slice(&32_u16.to_le_bytes());
        icon.extend_from_slice(&size.to_le_bytes());
        icon.extend_from_slice(&offset.to_le_bytes());
    }

    #[test]
    fn two_image_res_has_expected_records_and_alignment() {
        let resource = icon_to_res(&synthetic_icon()).expect("synthetic ICO should convert");
        let mut record_offsets = Vec::new();
        let mut cursor = 0;
        while cursor < resource.len() {
            record_offsets.push(cursor);
            let data_size = usize::try_from(read_u32(&resource, cursor))
                .expect("synthetic resource size fits usize");
            let header_size = usize::try_from(read_u32(&resource, cursor + 4))
                .expect("synthetic header size fits usize");
            cursor += header_size + data_size.next_multiple_of(4);
        }
        assert_eq!(record_offsets, [0, 32, 68, 108]);
        assert_eq!(resource.len(), 176);
        assert_eq!((read_u32(&resource, 0), read_u32(&resource, 4)), (0, 32));
        assert_eq!((read_u16(&resource, 10), read_u16(&resource, 14)), (0, 0));
        assert!(resource[16..32].iter().all(|byte| *byte == 0));

        let first = 32;
        assert_eq!(
            (read_u32(&resource, first), read_u32(&resource, first + 4)),
            (3, 32)
        );
        assert_eq!(
            (
                read_u16(&resource, first + 10),
                read_u16(&resource, first + 14)
            ),
            (3, 1)
        );
        assert_eq!(&resource[first + 32..first + 35], &[1, 2, 3]);

        let second = 68;
        assert_eq!(
            (read_u32(&resource, second), read_u32(&resource, second + 4)),
            (5, 32)
        );
        assert_eq!(
            (
                read_u16(&resource, second + 10),
                read_u16(&resource, second + 14)
            ),
            (3, 2)
        );

        let group = 108;
        assert_eq!(
            (read_u32(&resource, group), read_u32(&resource, group + 4)),
            (34, 32)
        );
        assert_eq!(
            (
                read_u16(&resource, group + 10),
                read_u16(&resource, group + 14)
            ),
            (14, 1)
        );
        assert_eq!(read_u16(&resource, group + 36), 2);
        assert_eq!(
            (
                read_u16(&resource, group + 50),
                read_u16(&resource, group + 64)
            ),
            (1, 2)
        );
        assert_eq!(first % 4, 0);
        assert_eq!(second % 4, 0);
        assert_eq!(group % 4, 0);
        assert_eq!(resource.len() % 4, 0);
    }

    #[test]
    fn rejects_wrong_icon_type() {
        let error = icon_to_res(&[0, 0, 2, 0, 1, 0]).expect_err("wrong type must fail");
        assert_eq!(error, "ICO type field must be 1, found 2");
    }

    #[test]
    fn rejects_truncated_directory_entry() {
        let error = icon_to_res(&[0, 0, 1, 0, 1, 0, 16]).expect_err("truncation must fail");
        assert!(error.contains("truncated in its directory"), "{error}");
    }
}
