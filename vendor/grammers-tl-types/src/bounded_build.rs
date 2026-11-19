// Run the pinned upstream build, then route its recursive decoder allocations
// through the cursor. The upstream script and generator remain source-identical.
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

mod upstream {
    include!("../build.rs");

    pub(super) fn generate() -> std::io::Result<()> {
        main()
    }
}

const DESERIALIZE_SIGNATURE: &str =
    "fn deserialize(buf: crate::deserialize::Buffer) -> crate::deserialize::Result<Self> {";
const EMPTY_DESERIALIZE_SIGNATURE: &str =
    "fn deserialize(_buf: crate::deserialize::Buffer) -> crate::deserialize::Result<Self> {";

fn invalid_format() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unexpected pinned TL decoder format",
    )
}

enum DecoderSelection {
    AllDeclarations,
    SendMessage,
    None,
}

fn bound_box_allocations(code: &str, selection: DecoderSelection) -> io::Result<(String, usize)> {
    let mut result = String::with_capacity(code.len());
    let mut enum_indent = None;
    let mut decoder = None;
    let mut expected_decoders = 0;
    let mut decoder_impls = 0;
    let mut boxed_variants = BTreeSet::new();
    let mut rewritten_variants = BTreeSet::new();
    for line in code.split_inclusive('\n') {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start_matches(' ').len();
        if trimmed.starts_with("pub struct ") || trimmed.starts_with("pub enum ") {
            expected_decoders += usize::from(match selection {
                DecoderSelection::AllDeclarations => true,
                // The upstream generator always decodes this request for update handling.
                DecoderSelection::SendMessage => trimmed.starts_with("pub struct SendMessage {"),
                DecoderSelection::None => false,
            });
        }
        if trimmed.starts_with("pub enum ") {
            enum_indent = Some(indent);
        } else if enum_indent == Some(indent) && trimmed == "}" {
            enum_indent = None;
        } else if enum_indent.is_some()
            && trimmed.contains("Box")
            && trimmed
                .split_ascii_whitespace()
                .collect::<String>()
                .contains("Box<")
        {
            let (variant, ty) = trimmed.split_once("(Box<").ok_or_else(invalid_format)?;
            let ty = ty.strip_suffix(">),").ok_or_else(invalid_format)?;
            if !boxed_variants.insert((variant, ty)) {
                return Err(invalid_format());
            }
        }

        if trimmed.starts_with("impl") && trimmed.contains(" crate::Deserializable for ") {
            if decoder.is_some()
                || !trimmed.ends_with('{')
                || (matches!(selection, DecoderSelection::SendMessage)
                    && !trimmed.ends_with(" crate::Deserializable for SendMessage {"))
            {
                return Err(invalid_format());
            }
            decoder = Some((indent, false));
            decoder_impls += 1;
        } else if let Some((impl_indent, signature_seen)) = &mut decoder {
            if indent == *impl_indent && trimmed == "}" {
                if !*signature_seen {
                    return Err(invalid_format());
                }
                decoder = None;
            } else if indent == *impl_indent + 4 && trimmed.starts_with("fn ") {
                if *signature_seen
                    || !matches!(trimmed, DESERIALIZE_SIGNATURE | EMPTY_DESERIALIZE_SIGNATURE)
                {
                    return Err(invalid_format());
                }
                *signature_seen = true;
            }
        }

        if decoder.is_some()
            && trimmed.contains("Box")
            && trimmed
                .split_ascii_whitespace()
                .collect::<String>()
                .contains("Box::")
        {
            let (prefix, allocation) = line.split_once("Box::new(").ok_or_else(invalid_format)?;
            let (ty, suffix) = allocation
                .split_once("::deserialize(buf)?)")
                .ok_or_else(invalid_format)?;
            let (constructor, variant) = prefix
                .trim()
                .split_once("::CONSTRUCTOR_ID => Self::")
                .ok_or_else(invalid_format)?;
            let variant = variant.strip_suffix('(').ok_or_else(invalid_format)?;
            if constructor != ty
                || !ty.starts_with("crate::types::")
                || !ty
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_'))
                || suffix != "),\n"
                || !rewritten_variants.insert((variant, ty))
            {
                return Err(invalid_format());
            }
            result.push_str(prefix);
            result.push_str("buf.read_box::<");
            result.push_str(ty);
            result.push_str(">()?");
            result.push_str(suffix);
        } else {
            result.push_str(line);
        }
    }
    if decoder.is_some()
        || enum_indent.is_some()
        || decoder_impls != expected_decoders
        || boxed_variants != rewritten_variants
    {
        return Err(invalid_format());
    }
    Ok((result, rewritten_variants.len()))
}

fn main() -> io::Result<()> {
    for input in [
        "build.rs",
        "src/bounded_build.rs",
        "tl/api.tl",
        "tl/mtproto.tl",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    upstream::generate()?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(invalid_format)?);
    for file in [
        "generated_common.rs",
        "generated_types.rs",
        "generated_functions.rs",
        "generated_enums.rs",
    ] {
        let path = output.join(file);
        let original = fs::read_to_string(&path)?;
        let selection = match file {
            "generated_common.rs" => DecoderSelection::None,
            "generated_functions.rs" if !cfg!(feature = "deserializable-functions") => {
                DecoderSelection::SendMessage
            }
            _ => DecoderSelection::AllDeclarations,
        };
        let (bounded, replacements) = bound_box_allocations(&original, selection)
            .map_err(|error| io::Error::new(error.kind(), format!("{file}: {error}")))?;
        let expected = if file == "generated_enums.rs" && cfg!(feature = "tl-api") {
            37
        } else {
            0
        };
        if replacements != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "pinned TL decoder allocation count changed in {file} ({replacements} instead of {expected})"
                ),
            ));
        }
        if replacements != 0 {
            fs::write(path, bounded)?;
        }
    }
    Ok(())
}
