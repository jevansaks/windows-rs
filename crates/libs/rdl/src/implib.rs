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

/// The resolved entry point named by a COFF short-import contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportEntryPoint {
    Name(String),
    Ordinal(u16),
}

/// One complete COFF short-import contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportContract {
    pub symbol: String,
    pub dll: String,
    /// Raw `IMAGE_FILE_MACHINE_*` value from the short-import header.
    pub machine: u16,
    pub import_type: ImportType,
    pub import_name: ImportName,
}

impl ImportContract {
    /// Resolves the PE/COFF import name mode without changing the source contract.
    ///
    /// `Name` preserves the raw symbol, `ExportAs` uses its explicit export, `NameNoPrefix`
    /// removes one leading `?`, `@`, or `_`, and `NameUndecorate` also truncates at the first `@`.
    /// Named modes reject an empty result.
    pub fn resolve_entry_point(&self) -> Result<ImportEntryPoint, Error> {
        let name = match &self.import_name {
            ImportName::Ordinal(ordinal) => return Ok(ImportEntryPoint::Ordinal(*ordinal)),
            ImportName::Name => self.symbol.as_str(),
            ImportName::ExportAs(name) => name,
            ImportName::NameNoPrefix => strip_import_prefix(&self.symbol),
            ImportName::NameUndecorate => {
                let name = strip_import_prefix(&self.symbol);
                name.split_once('@').map_or(name, |(name, _)| name)
            }
        };
        if name.is_empty() {
            return Err(err("short import resolves to an empty entry-point name"));
        }
        Ok(ImportEntryPoint::Name(name.to_string()))
    }
}

const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";
const MEMBER_HEADER_LEN: usize = 60;
/// `IMPORT_OBJECT_HEADER` short-import signature.
const IMPORT_SIGNATURE: &[u8] = &[0x00, 0x00, 0xFF, 0xFF];
const IMPORT_HEADER_LEN: usize = 20;
const VERSION_OFFSET: usize = 4;
const MACHINE_OFFSET: usize = 6;
const SIZE_OF_DATA_OFFSET: usize = 12;
const ORDINAL_OR_HINT_OFFSET: usize = 16;
const TYPE_INFO_OFFSET: usize = 18;

/// Parses every short-import member, preserving archive order and duplicates.
pub fn read(bytes: &[u8]) -> Result<Vec<Import>, Error> {
    read_archive(bytes, false, |data| {
        let mut parts = short_import_strings(data)?.split(|&b| b == 0);
        Ok(Import {
            symbol: next_string(&mut parts, "symbol")?,
            dll: next_string(&mut parts, "DLL")?,
        })
    })
}

/// Parses every complete short-import contract, preserving archive order and duplicates.
///
/// Unlike [`read`], this rejects incomplete archive tails and unterminated name data.
pub fn read_contracts(bytes: &[u8]) -> Result<Vec<ImportContract>, Error> {
    read_archive(bytes, true, parse_short_import)
}

fn read_archive<T>(
    bytes: &[u8],
    strict: bool,
    parse: impl Fn(&[u8]) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    if bytes.len() < ARCHIVE_MAGIC.len() || &bytes[..ARCHIVE_MAGIC.len()] != ARCHIVE_MAGIC {
        return Err(err("not a COFF archive (missing `!<arch>` magic)"));
    }

    let mut imports = vec![];
    let mut pos = ARCHIVE_MAGIC.len();

    loop {
        if pos >= bytes.len() {
            break;
        }
        if bytes.len() - pos < MEMBER_HEADER_LEN {
            if strict {
                return Err(err("truncated archive member header"));
            }
            break;
        }
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
        if name != b"/" && name != b"//" && is_short_import(data) {
            imports.push(parse(data)?);
        }

        pos = data_end + (size & 1);
        if strict && pos > bytes.len() {
            return Err(err("archive member is missing its padding byte"));
        }
    }

    Ok(imports)
}

fn parse_short_import(data: &[u8]) -> Result<ImportContract, Error> {
    let strings = short_import_strings(data)?;
    if strings.last() != Some(&0) {
        return Err(err("short import names are not NUL-terminated"));
    }
    let mut parts = strings.split(|&b| b == 0);
    let symbol = next_string(&mut parts, "symbol")?;
    let dll = next_string(&mut parts, "DLL")?;

    let machine = u16::from_le_bytes(
        data.get(MACHINE_OFFSET..MACHINE_OFFSET + 2)
            .ok_or_else(|| err("short import member is shorter than its header"))?
            .try_into()
            .unwrap(),
    );
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
        machine,
        import_type,
        import_name,
    })
}

fn is_short_import(data: &[u8]) -> bool {
    if !data.starts_with(IMPORT_SIGNATURE) {
        return false;
    }

    // Anonymous and BigObj headers share Sig1/Sig2 but use a nonzero Version.
    data.get(VERSION_OFFSET..VERSION_OFFSET + 2)
        .is_none_or(|version| version == [0, 0])
}

fn short_import_strings(data: &[u8]) -> Result<&[u8], Error> {
    let size_of_data = u32::from_le_bytes(
        data.get(SIZE_OF_DATA_OFFSET..SIZE_OF_DATA_OFFSET + 4)
            .ok_or_else(|| err("short import member is shorter than its header"))?
            .try_into()
            .unwrap(),
    ) as usize;

    let end = IMPORT_HEADER_LEN
        .checked_add(size_of_data)
        .ok_or_else(|| err("short import names extend past member data"))?;
    data.get(IMPORT_HEADER_LEN..end)
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

fn strip_import_prefix(name: &str) -> &str {
    name.strip_prefix(['?', '@', '_']).unwrap_or(name)
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
            short_import("OrdinalApi", "ordinal.dll", ImportType::Code, 0, 660, None),
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
                    symbol: "OrdinalApi".to_string(),
                    dll: "ordinal.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Code,
                    import_name: ImportName::Ordinal(660),
                },
                ImportContract {
                    symbol: "NamedApi".to_string(),
                    dll: "named.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Code,
                    import_name: ImportName::Name,
                },
                ImportContract {
                    symbol: "DataApi".to_string(),
                    dll: "data.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Data,
                    import_name: ImportName::Ordinal(12),
                },
                ImportContract {
                    symbol: "DecoratedApi".to_string(),
                    dll: "export.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Code,
                    import_name: ImportName::ExportAs("ExportedApi".to_string()),
                },
                ImportContract {
                    symbol: "_NoPrefixApi".to_string(),
                    dll: "prefix.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Code,
                    import_name: ImportName::NameNoPrefix,
                },
                ImportContract {
                    symbol: "_RouterControl@4".to_string(),
                    dll: "router.dll".to_string(),
                    machine: 0x8664,
                    import_type: ImportType::Code,
                    import_name: ImportName::NameUndecorate,
                },
            ]
        );
        assert_eq!(
            read(&archive).unwrap(),
            [
                Import {
                    symbol: "OrdinalApi".to_string(),
                    dll: "ordinal.dll".to_string(),
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
    fn contracts_preserve_raw_machine_values() {
        let mut x86 = short_import("X86Api", "x86.dll", ImportType::Code, 1, 1, None);
        x86[MACHINE_OFFSET..MACHINE_OFFSET + 2].copy_from_slice(&0x014Cu16.to_le_bytes());
        let mut arm64 = short_import("Arm64Api", "arm64.dll", ImportType::Code, 1, 2, None);
        arm64[MACHINE_OFFSET..MACHINE_OFFSET + 2].copy_from_slice(&0xAA64u16.to_le_bytes());

        let contracts = read_contracts(&archive([x86, arm64])).unwrap();
        assert_eq!(contracts[0].machine, 0x014C);
        assert_eq!(contracts[1].machine, 0xAA64);
    }

    #[test]
    fn resolves_every_import_name_mode() {
        let contracts = [
            ImportContract {
                symbol: "_OrdinalApi@4".to_string(),
                dll: "ordinal.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::Ordinal(321),
            },
            ImportContract {
                symbol: "_ExactName".to_string(),
                dll: "name.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::Name,
            },
            ImportContract {
                symbol: "_IgnoredSymbol".to_string(),
                dll: "export.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::ExportAs("ExactExport".to_string()),
            },
            ImportContract {
                symbol: "_NoPrefix".to_string(),
                dll: "prefix.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameNoPrefix,
            },
            ImportContract {
                symbol: "?RouterUnregisterForPrintAsyncNotifications@@YAJPEAX@Z".to_string(),
                dll: "SPOOLSS.DLL".to_string(),
                machine: 0x8664,
                import_type: ImportType::Code,
                import_name: ImportName::NameUndecorate,
            },
        ];

        assert_eq!(
            contracts
                .iter()
                .map(ImportContract::resolve_entry_point)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            [
                ImportEntryPoint::Ordinal(321),
                ImportEntryPoint::Name("_ExactName".to_string()),
                ImportEntryPoint::Name("ExactExport".to_string()),
                ImportEntryPoint::Name("NoPrefix".to_string()),
                ImportEntryPoint::Name("RouterUnregisterForPrintAsyncNotifications".to_string()),
            ]
        );
        assert_eq!(contracts[4].machine, 0x8664);
        assert_eq!(
            contracts[4].symbol,
            "?RouterUnregisterForPrintAsyncNotifications@@YAJPEAX@Z"
        );
    }

    #[test]
    fn no_prefix_removes_exactly_one_supported_prefix() {
        for (symbol, expected) in [
            ("ExactName", "ExactName"),
            ("_UnderscoreName", "UnderscoreName"),
            ("?QuestionName", "QuestionName"),
            ("@AtName", "AtName"),
            ("__DoublePrefix", "_DoublePrefix"),
        ] {
            let contract = ImportContract {
                symbol: symbol.to_string(),
                dll: "prefix.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameNoPrefix,
            };
            assert_eq!(
                contract.resolve_entry_point().unwrap(),
                ImportEntryPoint::Name(expected.to_string())
            );
        }
    }

    #[test]
    fn undecorate_applies_no_prefix_then_truncates_at_first_at() {
        for (symbol, expected) in [
            ("ExactName", "ExactName"),
            ("_StdcallName@4", "StdcallName"),
            ("@FastcallName@8", "FastcallName"),
            ("?CppName@@YAHXZ", "CppName"),
            ("__DoublePrefix@4", "_DoublePrefix"),
        ] {
            let contract = ImportContract {
                symbol: symbol.to_string(),
                dll: "undecorate.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameUndecorate,
            };
            assert_eq!(
                contract.resolve_entry_point().unwrap(),
                ImportEntryPoint::Name(expected.to_string())
            );
        }
    }

    #[test]
    fn resolved_entry_point_cannot_be_empty() {
        for contract in [
            ImportContract {
                symbol: String::new(),
                dll: "name.dll".to_string(),
                machine: 0x8664,
                import_type: ImportType::Code,
                import_name: ImportName::Name,
            },
            ImportContract {
                symbol: "ignored".to_string(),
                dll: "export.dll".to_string(),
                machine: 0x8664,
                import_type: ImportType::Code,
                import_name: ImportName::ExportAs(String::new()),
            },
            ImportContract {
                symbol: "_".to_string(),
                dll: "prefix.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameNoPrefix,
            },
            ImportContract {
                symbol: "@".to_string(),
                dll: "undecorate.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameUndecorate,
            },
            ImportContract {
                symbol: "_@4".to_string(),
                dll: "undecorate.dll".to_string(),
                machine: 0x014C,
                import_type: ImportType::Code,
                import_name: ImportName::NameUndecorate,
            },
        ] {
            assert!(
                contract
                    .resolve_entry_point()
                    .unwrap_err()
                    .to_string()
                    .contains("empty entry-point name")
            );
        }
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

    #[test]
    fn contracts_require_terminal_nul() {
        let dll = archive([without_terminal_nul(short_import(
            "NamedApi",
            "named.dll",
            ImportType::Code,
            1,
            7,
            None,
        ))]);
        assert_eq!(read(&dll).unwrap()[0].dll, "named.dll");
        assert!(
            read_contracts(&dll)
                .unwrap_err()
                .to_string()
                .contains("short import names are not NUL-terminated")
        );

        let export = archive([without_terminal_nul(short_import(
            "DecoratedApi",
            "export.dll",
            ImportType::Code,
            4,
            3,
            Some("ExportedApi"),
        ))]);
        assert_eq!(read(&export).unwrap()[0].dll, "export.dll");
        assert!(
            read_contracts(&export)
                .unwrap_err()
                .to_string()
                .contains("short import names are not NUL-terminated")
        );
    }

    #[test]
    fn contracts_reject_a_truncated_final_member_header() {
        let mut archive = archive([short_import(
            "NamedApi",
            "named.dll",
            ImportType::Code,
            1,
            7,
            None,
        )]);
        archive.extend_from_slice(b"partial archive header");

        assert_eq!(read(&archive).unwrap().len(), 1);
        assert!(
            read_contracts(&archive)
                .unwrap_err()
                .to_string()
                .contains("truncated archive member header")
        );
    }

    #[test]
    fn anonymous_objects_are_not_short_imports() {
        let archive = archive([
            anonymous_object(1, 32),
            short_import("NamedApi", "named.dll", ImportType::Code, 1, 7, None),
            anonymous_object(2, 56),
        ]);

        let imports = read(&archive).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].symbol, "NamedApi");
        let contracts = read_contracts(&archive).unwrap();
        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].symbol, "NamedApi");
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

    fn without_terminal_nul(mut import: Vec<u8>) -> Vec<u8> {
        assert_eq!(import.pop(), Some(0));
        let size = (import.len() - IMPORT_HEADER_LEN) as u32;
        import[SIZE_OF_DATA_OFFSET..SIZE_OF_DATA_OFFSET + 4].copy_from_slice(&size.to_le_bytes());
        import
    }

    fn anonymous_object(version: u16, len: usize) -> Vec<u8> {
        let mut result = vec![0, 0, 0xFF, 0xFF];
        result.extend_from_slice(&version.to_le_bytes());
        result.extend_from_slice(&0x8664u16.to_le_bytes());
        result.resize(len, 0);
        result
    }
}
