// Copyright 2018-2020 Commonwealth Labs, Inc.
// This file is part of Edgeware.

// Edgeware is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Edgeware is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Edgeware.  If not, see <http://www.gnu.org/licenses/>.

//! Some configurable implementations as associated type for the substrate runtime.

use core::num::NonZeroI128;
use edgeware_primitives::Balance;
use sp_runtime::traits::{Convert, Saturating};
use sp_runtime::{Fixed128, Perquintill};
use frame_support::{traits::{OnUnbalanced, Currency, Get}, weights::Weight};
use crate::{Balances, System, Authorship, MaximumBlockWeight, NegativeImbalance};

pub struct Author;
impl OnUnbalanced<NegativeImbalance> for Author {
	fn on_nonzero_unbalanced(amount: NegativeImbalance) {
		Balances::resolve_creating(&Authorship::author(), amount);
	}
}

/// Struct that handles the conversion of Balance -> `u64`. This is used for staking's election
/// calculation.
pub struct CurrencyToVoteHandler;

impl CurrencyToVoteHandler {
	fn factor() -> Balance { (Balances::total_issuance() / u64::max_value() as Balance).max(1) }
}

impl Convert<Balance, u64> for CurrencyToVoteHandler {
	fn convert(x: Balance) -> u64 { (x / Self::factor()) as u64 }
}

impl Convert<u128, Balance> for CurrencyToVoteHandler {
	fn convert(x: u128) -> Balance { x * Self::factor() }
}

/// Convert from weight to balance via a simple coefficient multiplication
/// The associated type C encapsulates a constant in units of balance per weight
pub struct LinearWeightToFee<C>(sp_std::marker::PhantomData<C>);

impl<C: Get<Balance>> Convert<Weight, Balance> for LinearWeightToFee<C> {
	fn convert(w: Weight) -> Balance {
		// setting this to zero will disable the weight fee.
		let coefficient = C::get();
		Balance::from(w).saturating_mul(coefficient)
	}
}

/// Update the given multiplier based on the following formula
///
///   diff = (previous_block_weight - target_weight)/max_weight
///   v = 0.00004
///   next_weight = weight * (1 + (v * diff) + (v * diff)^2 / 2)
///
/// Where `target_weight` must be given as the `Get` implementation of the `T` generic type.
/// https://research.web3.foundation/en/latest/polkadot/Token%20Economics/#relay-chain-transaction-fees
pub struct TargetedFeeAdjustment<T>(sp_std::marker::PhantomData<T>);

impl<T: Get<Perquintill>> Convert<Fixed128, Fixed128> for TargetedFeeAdjustment<T> {
	fn convert(multiplier: Fixed128) -> Fixed128 {
		let block_weight = System::all_extrinsics_weight();
		let max_weight = MaximumBlockWeight::get();
		let target_weight = (T::get() * max_weight) as u128;
		let block_weight = block_weight as u128;

		// determines if the first_term is positive
		let positive = block_weight >= target_weight;
		let diff_abs = block_weight.max(target_weight) - block_weight.min(target_weight);
		// safe, diff_abs cannot exceed u64 and it can always be computed safely even with the lossy
		// `Fixed128::from_rational`.
		let diff = Fixed128::from_rational(
			diff_abs as i128,
			NonZeroI128::new(max_weight.max(1) as i128).unwrap(),
		);
		let diff_squared = diff.saturating_mul(diff);

		// 0.00004 = 4/100_000 = 40_000/10^9
		let v = Fixed128::from_rational(4, NonZeroI128::new(100_000).unwrap());
		// 0.00004^2 = 16/10^10 Taking the future /2 into account... 8/10^10
		let v_squared_2 = Fixed128::from_rational(8, NonZeroI128::new(10_000_000_000).unwrap());

		let first_term = v.saturating_mul(diff);
		let second_term = v_squared_2.saturating_mul(diff_squared);

		if positive {
			// Note: this is merely bounded by how big the multiplier and the inner value can go,
			// not by any economical reasoning.
			let excess = first_term.saturating_add(second_term);
			multiplier.saturating_add(excess)
		} else {
			// Defensive-only: first_term > second_term. Safe subtraction.
			let negative = first_term.saturating_sub(second_term);
			multiplier.saturating_sub(negative)
				// despite the fact that apply_to saturates weight (final fee cannot go below 0)
				// it is crucially important to stop here and don't further reduce the weight fee
				// multiplier. While at -1, it means that the network is so un-congested that all
				// transactions have no weight fee. We stop here and only increase if the network
				// became more busy.
				.max(Fixed128::from_natural(-1))
		}
	}
}
