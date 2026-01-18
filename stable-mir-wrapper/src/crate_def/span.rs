//! Source code location information

/// A span - a location in source code
///
/// Spans point to a specific location in the source code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// The span data (location information)
    pub data: SpanData,
}

impl Span {
    /// Create a new span
    pub fn new(data: SpanData) -> Self {
        Self { data }
    }

    /// Get a dummy span (no location)
    pub fn dummy() -> Self {
        Self {
            data: SpanData::dummy(),
        }
    }
}

/// Span data - the actual location information
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanData {
    /// The starting location
    pub lo: BytePos,

    /// The ending location
    pub hi: BytePos,
}

impl SpanData {
    /// Create a new span data
    pub fn new(lo: BytePos, hi: BytePos) -> Self {
        Self { lo, hi }
    }

    /// Get a dummy span data
    pub fn dummy() -> Self {
        Self {
            lo: BytePos(0),
            hi: BytePos(0),
        }
    }
}

/// A byte position in the source code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BytePos(pub u32);
