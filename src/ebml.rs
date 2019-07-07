use bytes::{BigEndian, ByteOrder, BufMut};
use std::error::Error as ErrorTrait;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{Cursor, Error as IoError, ErrorKind, Result as IoResult, Write, Seek, SeekFrom};
use futures::Async;

pub const EBML_HEAD_ID: u64 = 0x0A45DFA3;
pub const DOC_TYPE_ID: u64 = 0x0282;
pub const VOID_ID: u64 = 0x6C;

#[derive(Debug, PartialEq)]
pub enum EbmlError {
    CorruptVarint,
    UnknownElementId,
    UnknownElementLength,
    CorruptPayload,
}
impl Display for EbmlError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}", self.description())
    }
}
impl ErrorTrait for EbmlError {
    fn description(&self) -> &str {
        match self {
            &EbmlError::CorruptVarint => "EBML Varint could not be parsed",
            &EbmlError::UnknownElementId => "EBML element ID was \"unknown\"",
            &EbmlError::UnknownElementLength => "EBML element length was \"unknown\" for an element not allowing that",
            &EbmlError::CorruptPayload => "EBML element payload could not be parsed",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum WriteError {
    OutOfRange
}
impl Display for WriteError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match self {
            &WriteError::OutOfRange => write!(f, "EBML Varint out of range")
        }
    }
}
impl ErrorTrait for WriteError {
    fn description(&self) -> &str {
        match self {
            &WriteError::OutOfRange => "EBML Varint out of range"
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Varint {
    /// a numeric value
    Value(u64),
    /// the reserved "unknown" value
    Unknown
}

/// Try to parse an EBML varint at the start of the given slice.
/// Returns an Err() if the format is corrupt.
/// Returns Ok(None) if more bytes are needed to get a result.
/// Returns Ok(Some((varint, size))) to return a varint value and
/// the size of the parsed varint.
pub fn decode_varint(bytes: &[u8]) -> Result<Option<(Varint, usize)>, EbmlError> {
    let mut value: u64 = 0;
    let mut value_length = 1;
    let mut mask: u8 = 0x80;
    let mut unknown_marker: u64 = !0;

    if bytes.len() == 0 {
        return Ok(None)
    }

    // get length marker bit from first byte & parse first byte
    while mask > 0 {
        if (mask & bytes[0]) != 0 {
            value = (bytes[0] & !mask) as u64;
            unknown_marker = (mask - 1) as u64;
            break
        }
        value_length += 1;
        mask = mask >> 1;
    }

    if mask == 0 {
        return Err(EbmlError::CorruptVarint)
    }

    // check we have enough data to parse
    if value_length > bytes.len() {
        return Ok(None)
    }

    // decode remaining bytes
    for i in 1..value_length {
        value = (value << 8) + (bytes[i] as u64);
        unknown_marker = (unknown_marker << 8) + 0xFF;
    }

    // determine result
    if value == unknown_marker {
        Ok(Some((Varint::Unknown, value_length)))
    } else {
        Ok(Some((Varint::Value(value), value_length)))
    }
}

/// Try to parse an EBML element header at the start of the given slice.
/// Returns an Err() if the format is corrupt.
/// Returns Ok(None) if more bytes are needed to get a result.
/// Returns Ok(Some((id, varint, size))) to return the element id,
/// the size of the payload, and the size of the parsed header.
pub fn decode_tag(bytes: &[u8]) -> Result<Option<(u64, Varint, usize)>, EbmlError> {
    // parse element ID
    match decode_varint(bytes) {
        Ok(None) => Ok(None),
        Err(err) => Err(err),
        Ok(Some((Varint::Unknown, _))) => Err(EbmlError::UnknownElementId),
        Ok(Some((Varint::Value(element_id), id_size))) => {
            // parse payload size
            match decode_varint(&bytes[id_size..]) {
                Ok(None) => Ok(None),
                Err(err) => Err(err),
                Ok(Some((element_length, length_size))) =>
                    Ok(Some((
                        element_id,
                        element_length,
                        id_size + length_size
                    )))
            }
        }
    }
}

pub fn decode_uint(bytes: &[u8]) -> Result<u64, EbmlError> {
    if bytes.len() < 1 || bytes.len() > 8 {
        return Err(EbmlError::CorruptPayload);
    }

    Ok(BigEndian::read_uint(bytes, bytes.len()))
}

const SMALL_FLAG: u64 = 0x80;
const EIGHT_FLAG: u64 = 0x01 << (8*7);
const EIGHT_MAX: u64 = EIGHT_FLAG - 2;

/// Tries to write an EBML varint using minimal space
pub fn encode_varint<T: Write>(varint: Varint, output: &mut T) -> IoResult<()> {
    let (size, number) = match varint {
        Varint::Unknown => (1, 0xFF),
        Varint::Value(too_big) if too_big > EIGHT_MAX => {
            return Err(IoError::new(ErrorKind::InvalidInput, WriteError::OutOfRange))
        },
        Varint::Value(value) => {
            let mut flag = SMALL_FLAG;
            let mut size = 1;
            // flag bit - 1 = UNKNOWN representation once OR'd with the flag;
            // if we're less than that, we can OR with the flag bit to get a valid Varint
            while value >= (flag - 1) {
                // right shift length bit by 1 to indicate adding a new byte;
                // left shift by 8 because there's a new byte at the end
                flag = flag << (8 - 1);
                size += 1;
            };
            (size, flag | value)
        }
    };

    let mut buffer = Cursor::new([0; 8]);
    buffer.put_uint_be(number, size);

    return output.write_all(&buffer.get_ref()[..size]);
}

const FOUR_FLAG: u64 = 0x10 << (8*3);
const FOUR_MAX: u64 = FOUR_FLAG - 2;

// tries to write a varint with a fixed 4-byte representation
pub fn encode_varint_4<T: Write>(varint: Varint, output: &mut T) -> IoResult<()> {
    let number = match varint {
        Varint::Unknown => FOUR_FLAG | (FOUR_FLAG - 1),
        Varint::Value(too_big) if too_big > FOUR_MAX => {
            return Err(IoError::new(ErrorKind::InvalidInput, WriteError::OutOfRange))
        },
        Varint::Value(value) => FOUR_FLAG | value
    };

    let mut buffer = Cursor::new([0; 4]);
    buffer.put_u32_be(number as u32);

    output.write_all(&buffer.get_ref()[..])
}

pub fn encode_element<T: Write + Seek, F: Fn(&mut T) -> IoResult<X>, X>(tag: u64, output: &mut T, content: F) -> IoResult<()> {
    encode_varint(Varint::Value(tag), output)?;
    encode_varint_4(Varint::Unknown, output)?;

    let start = output.seek(SeekFrom::Current(0))?;
    content(output)?;
    let end = output.seek(SeekFrom::Current(0))?;

    output.seek(SeekFrom::Start(start - 4))?;
    encode_varint_4(Varint::Value(end - start), output)?;
    output.seek(SeekFrom::Start(end))?;

    Ok(())
}

pub fn encode_tag_header<T: Write>(tag: u64, size: Varint, output: &mut T) -> IoResult<()> {
    encode_varint(Varint::Value(tag), output)?;
    encode_varint(size, output)
}

/// Tries to write a simple EBML tag with a string or binary value
pub fn encode_bytes<T: Write>(tag: u64, bytes: &[u8], output: &mut T) -> IoResult<()> {
    encode_tag_header(tag, Varint::Value(bytes.len() as u64), output)?;
    output.write_all(bytes)
}

/// Tries to write a simple EBML tag with an integer value
pub fn encode_integer<T: Write>(tag: u64, value: u64, output: &mut T) -> IoResult<()> {
    encode_tag_header(tag, Varint::Value(8), output)?;

    let mut buffer = Cursor::new([0; 8]);
    buffer.put_u64_be(value);

    output.write_all(&buffer.get_ref()[..])
}

pub trait FromEbml<'a>: Sized {
    /// Indicates if this tag's contents should be treated as a blob,
    /// or if the tag header should be reported as an event and with further
    /// parsing descending into its content.
    ///
    /// Unknown-size tags can *only* be parsed if unwrapped, and will error otherwise.
    fn should_unwrap(element_id: u64) -> bool;

    /// Given an element's ID and its binary payload, if any, construct a suitable
    /// instance of this type to represent the event. The instance may contain
    /// references into the given buffer.
    fn decode(element_id: u64, bytes: &'a[u8]) -> Result<Self, EbmlError>;

    /// Check if enough space exists in the given buffer for decode_element() to
    /// be successful; parsing errors will be returned eagerly.
    fn check_space(bytes: &[u8]) -> Result<Option<usize>, EbmlError> {
        match decode_tag(bytes) {
            Ok(None) => Ok(None),
            Err(err) => Err(err),
            Ok(Some((element_id, payload_size_tag, tag_size))) => {
                let should_unwrap = Self::should_unwrap(element_id);

                let payload_size = match (should_unwrap, payload_size_tag) {
                    (true, _) => 0,
                    (false, Varint::Unknown) => return Err(EbmlError::UnknownElementLength),
                    (false, Varint::Value(size)) => size as usize
                };

                let element_size = tag_size + payload_size;
                if element_size > bytes.len() {
                    // need to read more still
                    Ok(None)
                } else {
                    Ok(Some(element_size))
                }
            }
        }
    }

    /// Attempt to construct an instance of this type from the given byte slice
    fn decode_element(bytes: &'a[u8]) -> Result<Option<(Self, usize)>, EbmlError> {
        match decode_tag(bytes) {
            Ok(None) => Ok(None),
            Err(err) => Err(err),
            Ok(Some((element_id, payload_size_tag, tag_size))) => {
                let should_unwrap = Self::should_unwrap(element_id);

                let payload_size = match (should_unwrap, payload_size_tag) {
                    (true, _) => 0,
                    (false, Varint::Unknown) => return Err(EbmlError::UnknownElementLength),
                    (false, Varint::Value(size)) => size as usize
                };

                let element_size = tag_size + payload_size;
                if element_size > bytes.len() {
                    // need to read more still
                    return Ok(None);
                }

                match Self::decode(element_id, &bytes[tag_size..element_size]) {
                    Ok(element) => Ok(Some((element, element_size))),
                    Err(error) => Err(error)
                }
            }
        }
    }
}

pub trait EbmlEventSource {
    type Error;
    fn poll_event<'a, T: FromEbml<'a>>(&'a mut self) -> Result<Async<Option<T>>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use bytes::{BytesMut};
    use crate::ebml::*;
    use crate::ebml::EbmlError::{CorruptVarint, UnknownElementId};
    use crate::ebml::Varint::{Unknown, Value};
    use std::io::Cursor;
    use crate::tests::TEST_FILE;

    #[test]
    fn fail_corrupted_varints() {
        assert_eq!(decode_varint(&[0]), Err(CorruptVarint));
        assert_eq!(decode_varint(&[0, 0, 0]), Err(CorruptVarint));
    }

    #[test]
    fn incomplete_varints() {
        assert_eq!(decode_varint(&[]), Ok(None));
        assert_eq!(decode_varint(&[0x40]), Ok(None));
        assert_eq!(decode_varint(&[0x01, 0, 0]), Ok(None));
    }

    #[test]
    fn parse_varints() {
        assert_eq!(decode_varint(&[0xFF]), Ok(Some((Unknown, 1))));
        assert_eq!(decode_varint(&[0x7F, 0xFF]), Ok(Some((Unknown, 2))));
        assert_eq!(decode_varint(&[0x80]), Ok(Some((Value(0), 1))));
        assert_eq!(decode_varint(&[0x81]), Ok(Some((Value(1), 1))));
        assert_eq!(decode_varint(&[0x40, 52]), Ok(Some((Value(52), 2))));

        // test extra data in buffer
        assert_eq!(decode_varint(&[0x83, 0x11]), Ok(Some((Value(3), 1))));
    }

    #[test]
    fn encode_varints() {
        let mut buffer = BytesMut::with_capacity(10).writer();

        let mut no_space = Cursor::new([0; 0]).writer();
        assert_eq!(no_space.get_ref().remaining_mut(), 0);

        let mut six_buffer = Cursor::new([0; 6]).writer();
        assert_eq!(six_buffer.get_ref().remaining_mut(), 6);

        // 1 byte
        encode_varint(Varint::Unknown, &mut buffer).unwrap();
        assert_eq!(buffer.get_mut().split_to(1), &[0xFF].as_ref());
        assert_eq!(encode_varint(Varint::Unknown, &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        encode_varint(Varint::Value(0), &mut buffer).unwrap();
        assert_eq!(buffer.get_mut().split_to(1), &[0x80 | 0].as_ref());
        assert_eq!(encode_varint(Varint::Value(0), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        encode_varint(Varint::Value(1), &mut buffer).unwrap();
        assert_eq!(buffer.get_mut().split_to(1), &[0x80 | 1].as_ref());
        assert_eq!(encode_varint(Varint::Value(1), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        encode_varint(Varint::Value(126), &mut buffer).unwrap();
        assert_eq!(buffer.get_mut().split_to(1), &[0xF0 | 126].as_ref());
        assert_eq!(encode_varint(Varint::Value(126), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        // 2 bytes
        encode_varint(Varint::Value(127), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(2), &[0x40, 127].as_ref());
        assert_eq!(encode_varint(Varint::Value(127), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        encode_varint(Varint::Value(128), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(2), &[0x40, 128].as_ref());
        assert_eq!(encode_varint(Varint::Value(128), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);

        // 6 bytes
        assert_eq!(six_buffer.get_ref().remaining_mut(), 6);
        encode_varint(Varint::Value(0x03FFFFFFFFFE), &mut six_buffer).unwrap();
        assert_eq!(six_buffer.get_ref().remaining_mut(), 0);
        assert_eq!(&six_buffer.get_ref().get_ref(), &[0x07, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE].as_ref());
        six_buffer = Cursor::new([0; 6]).writer();

        // 7 bytes
        encode_varint(Varint::Value(0x03FFFFFFFFFF), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(7), &[0x02, 0x03, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF].as_ref());

        encode_varint(Varint::Value(0x01000000000000), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(7), &[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00].as_ref());

        encode_varint(Varint::Value(0x01FFFFFFFFFFFE), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(7), &[0x03, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE].as_ref());

        assert_eq!(encode_varint(Varint::Value(0x01FFFFFFFFFFFE), &mut no_space).unwrap_err().kind(), ErrorKind::WriteZero);
        assert_eq!(encode_varint(Varint::Value(0x01FFFFFFFFFFFE), &mut six_buffer).unwrap_err().kind(), ErrorKind::WriteZero);

        // 8 bytes
        encode_varint(Varint::Value(0x01FFFFFFFFFFFF), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(8), &[0x01, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF].as_ref());

        encode_varint(Varint::Value(0xFFFFFFFFFFFFFE), &mut buffer).unwrap();
        assert_eq!(&buffer.get_mut().split_to(8), &[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE].as_ref());

        assert_eq!(encode_varint(Varint::Value(0xFFFFFFFFFFFFFF), &mut buffer).unwrap_err().kind(), ErrorKind::InvalidInput);
        assert_eq!(encode_varint(Varint::Value(u64::max_value()), &mut buffer).unwrap_err().kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn fail_corrupted_tags() {
        assert_eq!(decode_tag(&[0]), Err(CorruptVarint));
        assert_eq!(decode_tag(&[0x80, 0]), Err(CorruptVarint));
        assert_eq!(decode_tag(&[0xFF, 0x80]), Err(UnknownElementId));
        assert_eq!(decode_tag(&[0x7F, 0xFF, 0x40, 0]), Err(UnknownElementId));
    }

    #[test]
    fn incomplete_tags() {
        assert_eq!(decode_tag(&[]), Ok(None));
        assert_eq!(decode_tag(&[0x80]), Ok(None));
        assert_eq!(decode_tag(&[0x40, 0, 0x40]), Ok(None));
    }

    #[test]
    fn parse_tags() {
        assert_eq!(decode_tag(&[0x80, 0x80]), Ok(Some((0, Value(0), 2))));
        assert_eq!(decode_tag(&[0x81, 0x85]), Ok(Some((1, Value(5), 2))));
        assert_eq!(decode_tag(&[0x80, 0xFF]), Ok(Some((0, Unknown, 2))));
        assert_eq!(decode_tag(&[0x80, 0x7F, 0xFF]), Ok(Some((0, Unknown, 3))));
        assert_eq!(decode_tag(&[0x85, 0x40, 52]), Ok(Some((5, Value(52), 3))));
    }

    #[test]
    fn bad_uints() {
        assert_eq!(decode_uint(&[]), Err(EbmlError::CorruptPayload));
        assert_eq!(decode_uint(&[0; 9]), Err(EbmlError::CorruptPayload));
    }

    #[test]
    fn parse_uints() {
        assert_eq!(decode_uint(&[0]), Ok(0));
        assert_eq!(decode_uint(&[0; 8]), Ok(0));
        assert_eq!(decode_uint(&[1]), Ok(1));
        assert_eq!(decode_uint(&[0,0,0,0,0,0,0,1]), Ok(1));
        assert_eq!(decode_uint(&[38]), Ok(38));
        assert_eq!(decode_uint(&[0,0,0,0,0,0,0,38]), Ok(38));
        assert_eq!(decode_uint(&[0x7F,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF]), Ok(9223372036854775807));
        assert_eq!(decode_uint(&[0x80,0,0,0,0,0,0,0]), Ok(9223372036854775808));
        assert_eq!(decode_uint(&[0x80,0,0,0,0,0,0,1]), Ok(9223372036854775809));
    }

    #[derive(Debug, PartialEq)]
    struct GenericElement(u64, usize);

    impl<'a> FromEbml<'a> for GenericElement {
        fn should_unwrap(element_id: u64) -> bool {
            match element_id {
                _ => false
            }
        }

        fn decode(element_id: u64, bytes: &'a[u8]) -> Result<GenericElement, EbmlError> {
            match element_id {
                _ => Ok(GenericElement(element_id, bytes.len()))
            }
        }
    }

    #[test]
    fn decode_sanity_test() {
        let decoded = GenericElement::decode_element(TEST_FILE);
        assert_eq!(decoded, Ok(Some((GenericElement(0x0A45DFA3, 31), 43))));
    }
}
