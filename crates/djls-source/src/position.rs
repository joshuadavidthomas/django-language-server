use serde::Serialize;
use thiserror::Error;

/// A byte offset within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Offset(u32);

impl Offset {
    #[must_use]
    pub fn new(offset: u32) -> Self {
        Self(offset)
    }

    #[must_use]
    pub fn get(&self) -> u32 {
        self.0
    }
}

impl From<u32> for Offset {
    #[inline]
    fn from(offset: u32) -> Self {
        Offset(offset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OffsetConversionError {
    #[error("value does not fit into u32")]
    Overflow,
}

impl TryFrom<usize> for Offset {
    type Error = OffsetConversionError;

    #[inline]
    fn try_from(offset: usize) -> Result<Self, Self::Error> {
        Ok(Self(
            u32::try_from(offset).map_err(|_| OffsetConversionError::Overflow)?,
        ))
    }
}

impl AsRef<u32> for Offset {
    #[inline]
    fn as_ref(&self) -> &u32 {
        &self.0
    }
}

impl std::borrow::Borrow<u32> for Offset {
    #[inline]
    fn borrow(&self) -> &u32 {
        &self.0
    }
}

impl core::fmt::Display for Offset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// A line and column position within a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineCol {
    line: u32,
    column: u32,
}

impl LineCol {
    #[must_use]
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }

    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    #[must_use]
    pub fn column(&self) -> u32 {
        self.column
    }
}

impl From<(u32, u32)> for LineCol {
    #[inline]
    fn from((line, column): (u32, u32)) -> Self {
        Self { line, column }
    }
}

impl From<LineCol> for (u32, u32) {
    #[inline]
    fn from(value: LineCol) -> Self {
        (value.line, value.column)
    }
}

impl From<&LineCol> for (u32, u32) {
    #[inline]
    fn from(value: &LineCol) -> Self {
        (value.line, value.column)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Span {
    start: u32,
    length: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, length: u32) -> Self {
        Self { start, length }
    }

    #[must_use]
    pub fn start(self) -> u32 {
        self.start
    }

    #[must_use]
    pub fn start_usize(self) -> usize {
        self.start as usize
    }

    #[must_use]
    pub fn length(self) -> u32 {
        self.length
    }

    #[must_use]
    pub fn length_usize(self) -> usize {
        self.length as usize
    }

    #[must_use]
    pub fn end(self) -> u32 {
        self.start.saturating_add(self.length)
    }

    #[must_use]
    pub fn start_offset(&self) -> Offset {
        Offset(self.start)
    }

    #[must_use]
    pub fn end_offset(&self) -> Offset {
        Offset(self.end())
    }

    #[must_use]
    pub fn with_length_usize_saturating(self, length: usize) -> Self {
        let max_length = u32::MAX.saturating_sub(self.start);
        let length_u32 = u32::try_from(length.min(max_length as usize)).unwrap_or(u32::MAX);
        Self {
            start: self.start,
            length: length_u32,
        }
    }

    #[must_use]
    pub fn saturating_from_parts_usize(start: usize, length: usize) -> Self {
        let start_u32 = u32::try_from(start.min(u32::MAX as usize)).unwrap_or(u32::MAX);
        let max_length = u32::MAX.saturating_sub(start_u32);
        let length_u32 = u32::try_from(length.min(max_length as usize)).unwrap_or(u32::MAX);
        Self {
            start: start_u32,
            length: length_u32,
        }
    }

    #[must_use]
    pub fn saturating_from_bounds_usize(start: usize, end: usize) -> Self {
        let s32 = u32::try_from(start.min(u32::MAX as usize)).unwrap_or(u32::MAX);
        let e32 = u32::try_from(end.min(u32::MAX as usize)).unwrap_or(u32::MAX);
        let (start_u32, end_u32) = if e32 >= s32 { (s32, e32) } else { (s32, s32) };
        Self {
            start: start_u32,
            length: end_u32 - start_u32,
        }
    }

    pub fn try_from_bounds_usize(start: usize, end: usize) -> Result<Self, SpanConversionError> {
        if end < start {
            return Err(SpanConversionError::EndBeforeStart);
        }
        let start_u32 = u32::try_from(start).map_err(|_| SpanConversionError::Overflow)?;
        let end_u32 = u32::try_from(end).map_err(|_| SpanConversionError::Overflow)?;
        Ok(Self {
            start: start_u32,
            length: end_u32 - start_u32,
        })
    }

    #[must_use]
    pub fn expand(self, opening: u32, closing: u32) -> Self {
        let start_expand = self.start.saturating_sub(opening);
        let length_expand = opening + self.length + closing;
        Self {
            start: start_expand,
            length: length_expand,
        }
    }

    #[must_use]
    pub fn contains(self, offset: Offset) -> bool {
        let offset_u32 = offset.get();
        offset_u32 >= self.start && offset_u32 < self.end()
    }
}

impl From<(u32, u32)> for Span {
    #[inline]
    fn from((start, length): (u32, u32)) -> Self {
        Self { start, length }
    }
}

impl From<Span> for (u32, u32) {
    #[inline]
    fn from(val: Span) -> Self {
        (val.start, val.length)
    }
}

impl From<&Span> for (u32, u32) {
    #[inline]
    fn from(val: &Span) -> Self {
        (val.start, val.length)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SpanConversionError {
    #[error("value does not fit into u32")]
    Overflow,
    #[error("end is before start")]
    EndBeforeStart,
}

impl TryFrom<(usize, usize)> for Span {
    type Error = SpanConversionError;

    #[inline]
    fn try_from((start, length): (usize, usize)) -> Result<Self, Self::Error> {
        Ok(Self {
            start: u32::try_from(start).map_err(|_| SpanConversionError::Overflow)?,
            length: u32::try_from(length).map_err(|_| SpanConversionError::Overflow)?,
        })
    }
}
