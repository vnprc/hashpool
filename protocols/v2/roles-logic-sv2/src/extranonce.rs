use binary_sv2::{B032, U256};
use core::{convert::TryInto, ops::Range};

pub const MAX_EXTRANONCE_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extranonce {
    extranonce: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendedExtranonceError {
    ExceedsMaxLength,
    InvalidRanges,
    MaxValueReached,
    InvalidDownstreamLength,
    InvalidStaticPrefixLength,
}

#[derive(Debug, Clone)]
pub struct ExtendedExtranonce {
    inner: Vec<u8>,
    range_0: Range<usize>,
    range_1: Range<usize>,
    range_2: Range<usize>,
    static_prefix: Option<Vec<u8>>,
}

impl From<Extranonce> for Vec<u8> {
    fn from(v: Extranonce) -> Self {
        v.extranonce
    }
}

impl<'a> From<U256<'a>> for Extranonce {
    fn from(v: U256<'a>) -> Self {
        let extranonce = v.inner_as_ref().into();
        Self { extranonce }
    }
}

impl From<Extranonce> for U256<'_> {
    fn from(v: Extranonce) -> Self {
        v.extranonce.try_into().expect("extranonce length checked")
    }
}

impl<'a> From<B032<'a>> for Extranonce {
    fn from(v: B032<'a>) -> Self {
        let extranonce = v.inner_as_ref().into();
        Self { extranonce }
    }
}

impl From<Extranonce> for B032<'_> {
    fn from(v: Extranonce) -> Self {
        v.extranonce.try_into().expect("extranonce length checked")
    }
}

impl TryFrom<Vec<u8>> for Extranonce {
    type Error = ();

    fn try_from(v: Vec<u8>) -> Result<Self, Self::Error> {
        if v.len() > MAX_EXTRANONCE_LEN {
            Err(())
        } else {
            Ok(Self { extranonce: v })
        }
    }
}

impl Default for Extranonce {
    fn default() -> Self {
        Self {
            extranonce: vec![0; MAX_EXTRANONCE_LEN],
        }
    }
}

impl Extranonce {
    pub fn new(len: usize) -> Option<Self> {
        if len > MAX_EXTRANONCE_LEN {
            None
        } else {
            Some(Self {
                extranonce: vec![0; len],
            })
        }
    }

    pub fn from_vec_with_len(mut extranonce: Vec<u8>, len: usize) -> Self {
        extranonce.resize(len, 0);
        Self { extranonce }
    }

    pub fn into_b032(self) -> B032<'static> {
        self.into()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<B032<'_>> {
        increment_bytes_be(&mut self.extranonce).ok()?;
        Some(
            self.extranonce
                .clone()
                .try_into()
                .expect("extranonce length checked"),
        )
    }

    pub fn to_vec(self) -> Vec<u8> {
        self.extranonce
    }
}

impl From<&mut ExtendedExtranonce> for Extranonce {
    fn from(v: &mut ExtendedExtranonce) -> Self {
        let mut extranonce = v.inner.to_vec();
        extranonce.truncate(v.range_2.end);
        Self { extranonce }
    }
}

impl PartialEq for ExtendedExtranonce {
    fn eq(&self, other: &Self) -> bool {
        let len = self.range_2.end;
        self.inner[0..len] == other.inner[0..len]
            && self.range_0 == other.range_0
            && self.range_1 == other.range_1
            && self.range_2 == other.range_2
    }
}

impl ExtendedExtranonce {
    pub fn new(
        range_0: Range<usize>,
        range_1: Range<usize>,
        range_2: Range<usize>,
        static_prefix: Option<Vec<u8>>,
    ) -> Result<Self, ExtendedExtranonceError> {
        Self::validate_ranges(&range_0, &range_1, &range_2)?;

        if let Some(static_prefix) = static_prefix.as_ref() {
            if static_prefix.len() > core::cmp::min(2, range_1.end - range_1.start) {
                return Err(ExtendedExtranonceError::InvalidStaticPrefixLength);
            }
        }

        let mut inner = vec![0; range_2.end];
        if let Some(static_prefix) = static_prefix.as_ref() {
            inner[range_1.start..range_1.start + static_prefix.len()]
                .copy_from_slice(static_prefix);
        }

        Ok(Self {
            inner,
            range_0,
            range_1,
            range_2,
            static_prefix,
        })
    }

    pub fn from_upstream_extranonce(
        v: Extranonce,
        range_0: Range<usize>,
        range_1: Range<usize>,
        range_2: Range<usize>,
    ) -> Result<Self, ExtendedExtranonceError> {
        Self::validate_ranges(&range_0, &range_1, &range_2)?;

        let mut inner = v.extranonce;
        inner.resize(range_2.end, 0);

        Ok(Self {
            inner,
            range_0,
            range_1,
            range_2,
            static_prefix: None,
        })
    }

    pub fn get_range2_len(&self) -> usize {
        self.range_2.end - self.range_2.start
    }

    pub fn get_range0_len(&self) -> usize {
        self.range_0.end - self.range_0.start
    }

    pub fn get_prefix_len(&self) -> usize {
        self.range_1.end - self.range_0.start
    }

    pub fn extranonce_from_downstream_extranonce(
        &self,
        downstream_extranonce: Extranonce,
    ) -> Result<Extranonce, ExtendedExtranonceError> {
        if downstream_extranonce.extranonce.len() != self.get_range2_len() {
            return Err(ExtendedExtranonceError::InvalidDownstreamLength);
        }

        let mut res = self.inner[self.range_0.start..self.range_1.end].to_vec();
        res.extend(downstream_extranonce.extranonce);
        res.try_into()
            .map_err(|_| ExtendedExtranonceError::ExceedsMaxLength)
    }

    pub fn next_prefix_standard(&mut self) -> Result<Extranonce, ExtendedExtranonceError> {
        let non_reserved_extranonces_bytes = &mut self.inner[self.range_2.start..self.range_2.end];
        increment_bytes_be(non_reserved_extranonces_bytes)
            .map_err(|_| ExtendedExtranonceError::MaxValueReached)?;
        Ok(self.into())
    }

    pub fn next_prefix_extended(
        &mut self,
        required_len: usize,
    ) -> Result<Extranonce, ExtendedExtranonceError> {
        if required_len > self.get_range2_len() {
            return Err(ExtendedExtranonceError::InvalidDownstreamLength);
        };

        let extended_part_start =
            self.range_1.start + self.static_prefix.as_ref().map_or(0, |p| p.len());

        increment_bytes_be(&mut self.inner[extended_part_start..self.range_1.end])
            .map_err(|_| ExtendedExtranonceError::MaxValueReached)?;
        self.inner[..self.range_1.end]
            .to_vec()
            .try_into()
            .map_err(|_| ExtendedExtranonceError::ExceedsMaxLength)
    }

    pub fn without_upstream_part(
        &self,
        downstream_extranonce: Option<Extranonce>,
    ) -> Result<Extranonce, ExtendedExtranonceError> {
        match downstream_extranonce {
            Some(downstream_extranonce) => {
                if downstream_extranonce.extranonce.len() != self.get_range2_len() {
                    return Err(ExtendedExtranonceError::InvalidDownstreamLength);
                }

                let mut res = self.inner[self.range_1.start..self.range_1.end].to_vec();
                res.extend(downstream_extranonce.extranonce);
                res.try_into()
                    .map_err(|_| ExtendedExtranonceError::ExceedsMaxLength)
            }
            None => self.inner[self.range_1.start..self.range_2.end]
                .to_vec()
                .try_into()
                .map_err(|_| ExtendedExtranonceError::ExceedsMaxLength),
        }
    }

    pub fn upstream_part(&self) -> Extranonce {
        Extranonce {
            extranonce: self.inner[self.range_0.start..self.range_1.end].to_vec(),
        }
    }

    fn validate_ranges(
        range_0: &Range<usize>,
        range_1: &Range<usize>,
        range_2: &Range<usize>,
    ) -> Result<(), ExtendedExtranonceError> {
        if range_0.start != 0
            || range_0.end != range_1.start
            || range_1.end != range_2.start
            || range_1.end < range_1.start
            || range_2.end < range_2.start
        {
            return Err(ExtendedExtranonceError::InvalidRanges);
        }

        if range_2.end > MAX_EXTRANONCE_LEN {
            return Err(ExtendedExtranonceError::ExceedsMaxLength);
        }

        Ok(())
    }
}

fn increment_bytes_be(bytes: &mut [u8]) -> Result<(), ()> {
    for byte in bytes.iter_mut().rev() {
        let (new_byte, overflow) = byte.overflowing_add(1);
        *byte = new_byte;
        if !overflow {
            return Ok(());
        }
    }
    Err(())
}
