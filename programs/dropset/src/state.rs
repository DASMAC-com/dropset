use anchor_lang_v2::prelude::*;

pub type FeeRate = u16;

pub const MAX_REGISTRY_ADMINS: usize = 3;
pub const MAX_REGISTRY_MAKERS: usize = 20;
pub const DEFAULT_MAX_SEATS_PER_MARKET: u8 = 10;

#[account]
pub struct Registry {
    pub default_taker_fee_rate: FeeRate,
    pub max_seats_per_market: u8,
    pub bump: u8,
    pub _pad: [u8; 2],
    pub admins: PodVec<Address, MAX_REGISTRY_ADMINS>,
    pub makers: PodVec<Address, MAX_REGISTRY_MAKERS>,
}
