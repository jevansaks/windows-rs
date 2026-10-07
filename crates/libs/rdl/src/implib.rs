//! Reader for SDK COFF import libraries, which record the DLL exporting each symbol.

use crate::Error;

/// One short-import symbol and its implementing DLL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub symbol: String,
    pub dll: String,
}

/// The kind of object described by a COFF short-import member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportType {
    Code,
    Data,
    Const,
}

/// The native entry-point contract described by a COFF short-import member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportName {
    Ordinal(u16),
    Name,
    NameNoPrefix,
    NameUndecorate,
    ExportAs(String),
}

/// One complete COFF short-import contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportContract {
    pub symbol: String,
    pub dll: String,
    pub import_type: ImportType,
    pub import_name: ImportName,
}

const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";
const MEMBER_HEADER_LEN: usize = 60;
/// `IMPORT_OBJECT_HEADER` short-import signature.
const IMPORT_SIGNATURE: &[u8] = &[0x00, 0x00, 0xFF, 0xFF];
const IMPORT_HEADER_LEN: usize = 20;
const SIZE_OF_DATA_OFFSET: usize = 12;
const ORDINAL_OR_HINT_OFFSET: usize = 16;
const TYPE_INFO_OFFSET: usize = 18;

/// Parses every short-import member, preserving archive order and duplicates.
pub fn read(bytes: &[u8]) -> Result<Vec<Import>, Error> {
    read_archive(bytes, |data| {
        let mut parts = short_import_strings(data)?.split(|&b| b == 0);
        Ok(Import {
            symbol: next_string(&mut parts, "symbol")?,
            dll: next_string(&mut parts, "DLL")?,
        })
    })
}

/// Parses every complete short-import contract, preserving archive order and duplicates.
pub fn read_contracts(bytes: &[u8]) -> Result<Vec<ImportContract>, Error> {
    read_archive(bytes, parse_short_import)
}

fn read_archive<T>(
    bytes: &[u8],
    parse: impl Fn(&[u8]) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    if bytes.len() < ARCHIVE_MAGIC.len() || &bytes[..ARCHIVE_MAGIC.len()] != ARCHIVE_MAGIC {
        return Err(err("not a COFF archive (missing `!<arch>` magic)"));
    }

    let mut imports = vec![];
    let mut pos = ARCHIVE_MAGIC.len();

    while pos + MEMBER_HEADER_LEN <= bytes.len() {
        let header = &bytes[pos..pos + MEMBER_HEADER_LEN];

        // The end marker guards against a misaligned archive walk.
        let name = trim(&header[0..16]);
        let size = parse_decimal(&header[48..58])?;
        if header[58..60] != [0x60, 0x0A] {
            return Err(err("malformed archive member header (bad end marker)"));
        }

        let data_start = pos + MEMBER_HEADER_LEN;
        let data_end = data_start
            .checked_add(size)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| err("archive member extends past end of data"))?;
        let data = &bytes[data_start..data_end];

        // Skip archive bookkeeping members.
        if name != b"/" && name != b"//" && data.starts_with(IMPORT_SIGNATURE) {
            imports.push(parse(data)?);
        }

        pos = data_end + (size & 1);
    }

    Ok(imports)
}

fn parse_short_import(data: &[u8]) -> Result<ImportContract, Error> {
    let mut parts = short_import_strings(data)?.split(|&b| b == 0);
    let symbol = next_string(&mut parts, "symbol")?;
    let dll = next_string(&mut parts, "DLL")?;

    let ordinal_or_hint = u16::from_le_bytes(
        data.get(ORDINAL_OR_HINT_OFFSET..ORDINAL_OR_HINT_OFFSET + 2)
            .ok_or_else(|| err("short import member is shorter than its header"))?
            .try_into()
            .unwrap(),
    );
    let type_info = u16::from_le_bytes(
        data.get(TYPE_INFO_OFFSET..TYPE_INFO_OFFSET + 2)
            .ok_or_else(|| err("short import member is shorter than its header"))?
            .try_into()
            .unwrap(),
    );
    let import_type = match type_info & 0b11 {
        0 => ImportType::Code,
        1 => ImportType::Data,
        2 => ImportType::Const,
        _ => return Err(err("short import has an invalid import type")),
    };
    let import_name = match (type_info >> 2) & 0b111 {
        0 => ImportName::Ordinal(ordinal_or_hint),
        1 => ImportName::Name,
        2 => ImportName::NameNoPrefix,
        3 => ImportName::NameUndecorate,
        4 => ImportName::ExportAs(next_string(&mut parts, "export")?),
        _ => return Err(err("short import has an invalid name type")),
    };

    Ok(ImportContract {
        symbol,
        dll,
        import_type,
        import_name,
    })
}

fn short_import_strings(data: &[u8]) -> Result<&[u8], Error> {
    let size_of_data = u32::from_le_bytes(
        data.get(SIZE_OF_DATA_OFFSET..SIZE_OF_DATA_OFFSET + 4)
            .ok_or_else(|| err("short import member is shorter than its header"))?
            .try_into()
            .unwrap(),
    ) as usize;

    data.get(IMPORT_HEADER_LEN..IMPORT_HEADER_LEN + size_of_data)
        .ok_or_else(|| err("short import names extend past member data"))
}

fn next_string<'a>(
    parts: &mut impl Iterator<Item = &'a [u8]>,
    what: &str,
) -> Result<String, Error> {
    let bytes = parts
        .next()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| err(&format!("short import missing {what} name")))?;
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| err(&format!("short import {what} name is not valid UTF-8")))
}

fn parse_decimal(field: &[u8]) -> Result<usize, Error> {
    let text = std::str::from_utf8(field)
        .map_err(|_| err("archive member size is not valid ASCII"))?
        .trim();
    text.parse::<usize>()
        .map_err(|_| err("archive member has an invalid size field"))
}

fn trim(field: &[u8]) -> &[u8] {
    let end = field.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    &field[..end]
}

fn err(message: &str) -> Error {
    Error::new(message, "", 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_complete_short_import_contracts() {
        let archive = archive([
            short_import(
                "FileIconInit",
                "shell32.dll",
                ImportType::Code,
                0,
                660,
                None,
            ),
            short_import("NamedApi", "named.dll", ImportType::Code, 1, 7, None),
            short_import("DataApi", "data.dll", ImportType::Data, 0, 12, None),
            short_import(
                "DecoratedApi",
                "export.dll",
                ImportType::Code,
                4,
                3,
                Some("ExportedApi"),
            ),
            short_import("_NoPrefixApi", "prefix.dll", ImportType::Code, 2, 4, None),
            short_import(
                "_RouterControl@4",
                "router.dll",
                ImportType::Code,
                3,
                5,
                None,
            ),
        ]);

        assert_eq!(
            read_contracts(&archive).unwrap(),
            [
                ImportContract {
                    symbol: "FileIconInit".to_string(),
                    dll: "shell32.dll".to_string(),
                    import_type: ImportType::Code,
                    import_name: ImportName::Ordinal(660),
                },
                ImportContract {
                    symbol: "NamedApi".to_string(),
                    dll: "named.dll".to_string(),
                    import_type: ImportType::Code,
                    import_name: ImportName::Name,
                },
                ImportContract {
                    symbol: "DataApi".to_string(),
                    dll: "data.dll".to_string(),
                    import_type: ImportType::Data,
                    import_name: ImportName::Ordinal(12),
                },
                ImportContract {
                    symbol: "DecoratedApi".to_string(),
                    dll: "export.dll".to_string(),
                    import_type: ImportType::Code,
                    import_name: ImportName::ExportAs("ExportedApi".to_string()),
                },
                ImportContract {
                    symbol: "_NoPrefixApi".to_string(),
                    dll: "prefix.dll".to_string(),
                    import_type: ImportType::Code,
                    import_name: ImportName::NameNoPrefix,
                },
                ImportContract {
                    symbol: "_RouterControl@4".to_string(),
                    dll: "router.dll".to_string(),
                    import_type: ImportType::Code,
                    import_name: ImportName::NameUndecorate,
                },
            ]
        );
        assert_eq!(
            read(&archive).unwrap(),
            [
                Import {
                    symbol: "FileIconInit".to_string(),
                    dll: "shell32.dll".to_string(),
                },
                Import {
                    symbol: "NamedApi".to_string(),
                    dll: "named.dll".to_string(),
                },
                Import {
                    symbol: "DataApi".to_string(),
                    dll: "data.dll".to_string(),
                },
                Import {
                    symbol: "DecoratedApi".to_string(),
                    dll: "export.dll".to_string(),
                },
                Import {
                    symbol: "_NoPrefixApi".to_string(),
                    dll: "prefix.dll".to_string(),
                },
                Import {
                    symbol: "_RouterControl@4".to_string(),
                    dll: "router.dll".to_string(),
                },
            ]
        );
    }

    #[test]
    fn export_as_requires_an_explicit_name() {
        let archive = archive([short_import(
            "DecoratedApi",
            "export.dll",
            ImportType::Code,
            4,
            3,
            None,
        )]);

        assert!(
            read_contracts(&archive)
                .unwrap_err()
                .to_string()
                .contains("short import missing export name")
        );
    }

    #[test]
    fn legacy_reader_ignores_unexposed_contract_bits() {
        let mut archive = archive([short_import(
            "LegacyApi",
            "legacy.dll",
            ImportType::Code,
            7,
            3,
            None,
        )]);
        let type_info: u16 = 3 | 7 << 2;
        let offset = ARCHIVE_MAGIC.len() + MEMBER_HEADER_LEN + TYPE_INFO_OFFSET;
        archive[offset..offset + 2].copy_from_slice(&type_info.to_le_bytes());

        assert_eq!(
            read(&archive).unwrap(),
            [Import {
                symbol: "LegacyApi".to_string(),
                dll: "legacy.dll".to_string(),
            }]
        );
        assert!(
            read_contracts(&archive)
                .unwrap_err()
                .to_string()
                .contains("invalid import type")
        );
    }

    fn archive<const N: usize>(members: [Vec<u8>; N]) -> Vec<u8> {
        let mut result = ARCHIVE_MAGIC.to_vec();
        for (index, member) in members.into_iter().enumerate() {
            let name = format!("member{index}/");
            let header = format!(
                "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                0,
                0,
                0,
                0,
                member.len()
            );
            assert_eq!(header.len(), MEMBER_HEADER_LEN);
            result.extend_from_slice(header.as_bytes());
            result.extend_from_slice(&member);
            if member.len() & 1 != 0 {
                result.push(b'\n');
            }
        }
        result
    }

    fn short_import(
        symbol: &str,
        dll: &str,
        import_type: ImportType,
        name_type: u16,
        ordinal_or_hint: u16,
        export: Option<&str>,
    ) -> Vec<u8> {
        let mut strings = vec![];
        for value in [Some(symbol), Some(dll), export].into_iter().flatten() {
            strings.extend_from_slice(value.as_bytes());
            strings.push(0);
        }
        let import_type = match import_type {
            ImportType::Code => 0,
            ImportType::Data => 1,
            ImportType::Const => 2,
        };
        let mut result = vec![0, 0, 0xFF, 0xFF, 0, 0, 0x64, 0x86, 0, 0, 0, 0];
        result.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        result.extend_from_slice(&ordinal_or_hint.to_le_bytes());
        result.extend_from_slice(&(import_type | name_type << 2).to_le_bytes());
        result.extend_from_slice(&strings);
        result
    }
}
