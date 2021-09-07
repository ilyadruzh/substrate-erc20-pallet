#![cfg_attr(not(feature = "std"), no_std)]

pub mod weights;

mod extra_mutator;
pub use extra_mutator::*;
mod functions;
mod impl_fungibles;
mod impl_stored_map;
mod types;
pub use types::*;

use codec::HasCompact;
use frame_support::{
	dispatch::{DispatchError, DispatchResult},
	ensure,
	traits::{
		tokens::{fungibles, DepositConsequence, WithdrawConsequence},
		BalanceStatus::Reserved,
		Currency, ReservableCurrency, StoredMap,
	},
};
use frame_system::Config as SystemConfig;
use sp_runtime::{
	traits::{
		AtLeast32BitUnsigned, Bounded, CheckedAdd, CheckedSub, Saturating, StaticLookup, Zero,
	},
	ArithmeticError, TokenError,
};
use sp_std::{borrow::Borrow, convert::TryInto, prelude::*};

pub use pallet::*;
pub use weights::WeightInfo;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{dispatch::DispatchResult, pallet_prelude::*};
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::generate_storage_info]
	pub struct Pallet<T, I = ()>(_);

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;
		type Balance: Member + Parameter + AtLeast32BitUnsigned + Default + Copy + MaxEncodedLen;
		type AssetId: Member + Parameter + Default + Copy + HasCompact + MaxEncodedLen;
		type Currency: ReservableCurrency<Self::AccountId>;
		type ForceOrigin: EnsureOrigin<Self::Origin>;

		#[pallet::constant]
		type AssetDeposit: Get<DepositBalanceOf<Self, I>>;

		#[pallet::constant]
		type MetadataDepositBase: Get<DepositBalanceOf<Self, I>>;

		#[pallet::constant]
		type MetadataDepositPerByte: Get<DepositBalanceOf<Self, I>>;

		#[pallet::constant]
		type ApprovalDeposit: Get<DepositBalanceOf<Self, I>>;

		#[pallet::constant]
		type StringLimit: Get<u32>;

		type Freezer: FrozenBalance<Self::AssetId, Self::AccountId, Self::Balance>;

		type Extra: Member + Parameter + Default + MaxEncodedLen;

		type WeightInfo: WeightInfo;
	}

	#[pallet::storage]
	pub(super) type Asset<T: Config<I>, I: 'static = ()> = StorageMap<
		_,
		Blake2_128Concat,
		T::AssetId,
		AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
	>;

	// ERC20: mapping(address => uint256) _balances
	#[pallet::storage]
	#[pallet::getter(fn balance_of)]
	pub(super) type Account<T: Config<I>, I: 'static = ()> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		T::AssetId,
		Blake2_128Concat,
		T::AccountId,
		AssetBalance<T::Balance, T::Extra>,
		ValueQuery,
		GetDefault,
		ConstU32<300_000>,
	>;

	// ERC20: mapping(address => mapping(address => uint256)) _allowances
	#[pallet::storage]
	pub(super) type Approvals<T: Config<I>, I: 'static = ()> = StorageNMap<
		_,
		(
			NMapKey<Blake2_128Concat, T::AssetId>,
			NMapKey<Blake2_128Concat, T::AccountId>, // owner
			NMapKey<Blake2_128Concat, T::AccountId>, // delegate
		),
		Approval<T::Balance, DepositBalanceOf<T, I>>,
		OptionQuery,
		GetDefault,
		ConstU32<300_000>,
	>;

	#[pallet::storage]
	pub(super) type Metadata<T: Config<I>, I: 'static = ()> = StorageMap<
		_,
		Blake2_128Concat,
		T::AssetId,
		AssetMetadata<DepositBalanceOf<T, I>, BoundedVec<u8, T::StringLimit>>,
		ValueQuery,
		GetDefault,
		ConstU32<300_000>,
	>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	#[pallet::metadata(T::AccountId = "AccountId", T::Balance = "Balance", T::AssetId = "AssetId")]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// Some asset class was created. \[asset_id, creator, owner\]
		Created(T::AssetId, T::AccountId, T::AccountId),
		/// Some assets were issued. \[asset_id, owner, total_supply\]
		Issued(T::AssetId, T::AccountId, T::Balance),
		/// Some assets were transferred. \[asset_id, from, to, amount\]
		Transferred(T::AssetId, T::AccountId, T::AccountId, T::Balance),
		/// Some assets were destroyed. \[asset_id, owner, balance\]
		Burned(T::AssetId, T::AccountId, T::Balance),
		/// The management team changed \[asset_id, issuer, admin, freezer\]
		TeamChanged(T::AssetId, T::AccountId, T::AccountId, T::AccountId),
		/// The owner changed \[asset_id, owner\]
		OwnerChanged(T::AssetId, T::AccountId),
		/// Some account `who` was frozen. \[asset_id, who\]
		Frozen(T::AssetId, T::AccountId),
		/// Some account `who` was thawed. \[asset_id, who\]
		Thawed(T::AssetId, T::AccountId),
		/// Some asset `asset_id` was frozen. \[asset_id\]
		AssetFrozen(T::AssetId),
		/// Some asset `asset_id` was thawed. \[asset_id\]
		AssetThawed(T::AssetId),
		/// An asset class was destroyed.
		Destroyed(T::AssetId),
		/// Some asset class was force-created. \[asset_id, owner\]
		ForceCreated(T::AssetId, T::AccountId),
		/// New metadata has been set for an asset. \[asset_id, name, symbol, decimals, is_frozen\]
		MetadataSet(T::AssetId, Vec<u8>, Vec<u8>, u8, bool),
		/// Metadata has been cleared for an asset. \[asset_id\]
		MetadataCleared(T::AssetId),
		/// (Additional) funds have been approved for transfer to a destination account.
		/// \[asset_id, source, delegate, amount\]
		ApprovedTransfer(T::AssetId, T::AccountId, T::AccountId, T::Balance),
		/// An approval for account `delegate` was cancelled by `owner`.
		/// \[id, owner, delegate\]
		ApprovalCancelled(T::AssetId, T::AccountId, T::AccountId),
		/// An `amount` was transferred in its entirety from `owner` to `destination` by
		/// the approved `delegate`.
		/// \[id, owner, delegate, destination\]
		TransferredApproved(T::AssetId, T::AccountId, T::AccountId, T::AccountId, T::Balance),
		/// An asset has had its attributes changed by the `Force` origin.
		/// \[id\]
		AssetStatusChanged(T::AssetId),
	}

	#[pallet::error]
	pub enum Error<T, I = ()> {
		/// Account balance must be greater than or equal to the transfer amount.
		BalanceLow,
		/// Balance should be non-zero.
		BalanceZero,
		/// The signing account has no permission to do the operation.
		NoPermission,
		/// The given asset ID is unknown.
		Unknown,
		/// The origin account is frozen.
		Frozen,
		/// The asset ID is already taken.
		InUse,
		/// Invalid witness data given.
		BadWitness,
		/// Minimum balance should be non-zero.
		MinBalanceZero,
		/// No provider reference exists to allow a non-zero balance of a non-self-sufficient
		/// asset.
		NoProvider,
		/// Invalid metadata given.
		BadMetadata,
		/// No approval exists that would allow the transfer.
		Unapproved,
		/// The source account would not survive the transfer and it needs to stay alive.
		WouldDie,
	}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		#[pallet::weight(T::WeightInfo::create())]
		pub fn create(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			admin: <T::Lookup as StaticLookup>::Source,
			min_balance: T::Balance,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let admin = T::Lookup::lookup(admin)?;

			ensure!(!Asset::<T, I>::contains_key(id), Error::<T, I>::InUse);
			ensure!(!min_balance.is_zero(), Error::<T, I>::MinBalanceZero);

			let deposit = T::AssetDeposit::get();
			T::Currency::reserve(&owner, deposit)?;

			Asset::<T, I>::insert(
				id,
				AssetDetails {
					owner: owner.clone(),
					issuer: admin.clone(),
					admin: admin.clone(),
					freezer: admin.clone(),
					supply: Zero::zero(),
					deposit,
					min_balance,
					is_sufficient: false,
					accounts: 0,
					sufficients: 0,
					approvals: 0,
					is_frozen: false,
				},
			);
			Self::deposit_event(Event::Created(id, owner, admin));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::force_create())]
		pub fn force_create(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			owner: <T::Lookup as StaticLookup>::Source,
			is_sufficient: bool,
			#[pallet::compact] min_balance: T::Balance,
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;
			let owner = T::Lookup::lookup(owner)?;

			ensure!(!Asset::<T, I>::contains_key(id), Error::<T, I>::InUse);
			ensure!(!min_balance.is_zero(), Error::<T, I>::MinBalanceZero);

			Asset::<T, I>::insert(
				id,
				AssetDetails {
					owner: owner.clone(),
					issuer: owner.clone(),
					admin: owner.clone(),
					freezer: owner.clone(),
					supply: Zero::zero(),
					deposit: Zero::zero(),
					min_balance,
					is_sufficient,
					accounts: 0,
					sufficients: 0,
					approvals: 0,
					is_frozen: false,
				},
			);
			Self::deposit_event(Event::ForceCreated(id, owner));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::destroy(
			witness.accounts.saturating_sub(witness.sufficients),
 			witness.sufficients,
 			witness.approvals,
 		))]
		pub fn destroy(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			witness: DestroyWitness,
		) -> DispatchResultWithPostInfo {
			let maybe_check_owner = match T::ForceOrigin::try_origin(origin) {
				Ok(_) => None,
				Err(origin) => Some(ensure_signed(origin)?),
			};
			Asset::<T, I>::try_mutate_exists(id, |maybe_details| {
				let mut details = maybe_details.take().ok_or(Error::<T, I>::Unknown)?;
				if let Some(check_owner) = maybe_check_owner {
					ensure!(details.owner == check_owner, Error::<T, I>::NoPermission);
				}
				ensure!(details.accounts <= witness.accounts, Error::<T, I>::BadWitness);
				ensure!(details.sufficients <= witness.sufficients, Error::<T, I>::BadWitness);
				ensure!(details.approvals <= witness.approvals, Error::<T, I>::BadWitness);

				for (who, v) in Account::<T, I>::drain_prefix(id) {
					Self::dead_account(id, &who, &mut details, v.sufficient);
				}
				debug_assert_eq!(details.accounts, 0);
				debug_assert_eq!(details.sufficients, 0);

				let metadata = Metadata::<T, I>::take(&id);
				T::Currency::unreserve(
					&details.owner,
					details.deposit.saturating_add(metadata.deposit),
				);

				for ((owner, _), approval) in Approvals::<T, I>::drain_prefix((&id,)) {
					T::Currency::unreserve(&owner, approval.deposit);
				}
				Self::deposit_event(Event::Destroyed(id));

				Ok(Some(T::WeightInfo::destroy(
					details.accounts.saturating_sub(details.sufficients),
					details.sufficients,
					details.approvals,
				))
				.into())
			})
		}

		#[pallet::weight(T::WeightInfo::mint())]
		pub fn mint(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			beneficiary: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;
			Self::do_mint(id, &beneficiary, amount, Some(origin))?;
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::burn())]
		pub fn burn(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			who: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let who = T::Lookup::lookup(who)?;

			let f = DebitFlags { keep_alive: false, best_effort: true };
			let _ = Self::do_burn(id, &who, amount, Some(origin), f)?;
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::transfer())]
		pub fn transfer(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			target: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(target)?;

			let f = TransferFlags { keep_alive: false, best_effort: false, burn_dust: false };
			Self::do_transfer(id, &origin, &dest, amount, None, f).map(|_| ())
		}

		#[pallet::weight(T::WeightInfo::transfer_keep_alive())]
		pub fn transfer_keep_alive(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			target: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let source = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(target)?;

			let f = TransferFlags { keep_alive: true, best_effort: false, burn_dust: false };
			Self::do_transfer(id, &source, &dest, amount, None, f).map(|_| ())
		}

		#[pallet::weight(T::WeightInfo::force_transfer())]
		pub fn force_transfer(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			source: <T::Lookup as StaticLookup>::Source,
			dest: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let source = T::Lookup::lookup(source)?;
			let dest = T::Lookup::lookup(dest)?;

			let f = TransferFlags { keep_alive: false, best_effort: false, burn_dust: false };
			Self::do_transfer(id, &source, &dest, amount, Some(origin), f).map(|_| ())
		}

		#[pallet::weight(T::WeightInfo::freeze())]
		pub fn freeze(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			who: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			let d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			ensure!(&origin == &d.freezer, Error::<T, I>::NoPermission);
			let who = T::Lookup::lookup(who)?;
			ensure!(Account::<T, I>::contains_key(id, &who), Error::<T, I>::BalanceZero);

			Account::<T, I>::mutate(id, &who, |a| a.is_frozen = true);

			Self::deposit_event(Event::<T, I>::Frozen(id, who));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::thaw())]
		pub fn thaw(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			who: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			let details = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			ensure!(&origin == &details.admin, Error::<T, I>::NoPermission);
			let who = T::Lookup::lookup(who)?;
			ensure!(Account::<T, I>::contains_key(id, &who), Error::<T, I>::BalanceZero);

			Account::<T, I>::mutate(id, &who, |a| a.is_frozen = false);

			Self::deposit_event(Event::<T, I>::Thawed(id, who));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::freeze_asset())]
		pub fn freeze_asset(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			Asset::<T, I>::try_mutate(id, |maybe_details| {
				let d = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;
				ensure!(&origin == &d.freezer, Error::<T, I>::NoPermission);

				d.is_frozen = true;

				Self::deposit_event(Event::<T, I>::AssetFrozen(id));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::thaw_asset())]
		pub fn thaw_asset(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			Asset::<T, I>::try_mutate(id, |maybe_details| {
				let d = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;
				ensure!(&origin == &d.admin, Error::<T, I>::NoPermission);

				d.is_frozen = false;

				Self::deposit_event(Event::<T, I>::AssetThawed(id));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::transfer_ownership())]
		pub fn transfer_ownership(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			owner: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let owner = T::Lookup::lookup(owner)?;

			Asset::<T, I>::try_mutate(id, |maybe_details| {
				let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;
				ensure!(&origin == &details.owner, Error::<T, I>::NoPermission);
				if details.owner == owner {
					return Ok(());
				}

				let metadata_deposit = Metadata::<T, I>::get(id).deposit;
				let deposit = details.deposit + metadata_deposit;

				// Move the deposit to the new owner.
				T::Currency::repatriate_reserved(&details.owner, &owner, deposit, Reserved)?;

				details.owner = owner.clone();

				Self::deposit_event(Event::OwnerChanged(id, owner));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::set_team())]
		pub fn set_team(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			issuer: <T::Lookup as StaticLookup>::Source,
			admin: <T::Lookup as StaticLookup>::Source,
			freezer: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let issuer = T::Lookup::lookup(issuer)?;
			let admin = T::Lookup::lookup(admin)?;
			let freezer = T::Lookup::lookup(freezer)?;

			Asset::<T, I>::try_mutate(id, |maybe_details| {
				let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;
				ensure!(&origin == &details.owner, Error::<T, I>::NoPermission);

				details.issuer = issuer.clone();
				details.admin = admin.clone();
				details.freezer = freezer.clone();

				Self::deposit_event(Event::TeamChanged(id, issuer, admin, freezer));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::set_metadata(name.len() as u32, symbol.len() as u32))]
		pub fn set_metadata(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			name: Vec<u8>,
			symbol: Vec<u8>,
			decimals: u8,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			let bounded_name: BoundedVec<u8, T::StringLimit> =
				name.clone().try_into().map_err(|_| Error::<T, I>::BadMetadata)?;
			let bounded_symbol: BoundedVec<u8, T::StringLimit> =
				symbol.clone().try_into().map_err(|_| Error::<T, I>::BadMetadata)?;

			let d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			ensure!(&origin == &d.owner, Error::<T, I>::NoPermission);

			Metadata::<T, I>::try_mutate_exists(id, |metadata| {
				ensure!(
					metadata.as_ref().map_or(true, |m| !m.is_frozen),
					Error::<T, I>::NoPermission
				);

				let old_deposit = metadata.take().map_or(Zero::zero(), |m| m.deposit);
				let new_deposit = T::MetadataDepositPerByte::get()
					.saturating_mul(((name.len() + symbol.len()) as u32).into())
					.saturating_add(T::MetadataDepositBase::get());

				if new_deposit > old_deposit {
					T::Currency::reserve(&origin, new_deposit - old_deposit)?;
				} else {
					T::Currency::unreserve(&origin, old_deposit - new_deposit);
				}

				*metadata = Some(AssetMetadata {
					deposit: new_deposit,
					name: bounded_name,
					symbol: bounded_symbol,
					decimals,
					is_frozen: false,
				});

				Self::deposit_event(Event::MetadataSet(id, name, symbol, decimals, false));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::clear_metadata())]
		pub fn clear_metadata(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;

			let d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			ensure!(&origin == &d.owner, Error::<T, I>::NoPermission);

			Metadata::<T, I>::try_mutate_exists(id, |metadata| {
				let deposit = metadata.take().ok_or(Error::<T, I>::Unknown)?.deposit;
				T::Currency::unreserve(&d.owner, deposit);
				Self::deposit_event(Event::MetadataCleared(id));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::force_set_metadata(name.len() as u32, symbol.len() as u32))]
		pub fn force_set_metadata(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			name: Vec<u8>,
			symbol: Vec<u8>,
			decimals: u8,
			is_frozen: bool,
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;

			let bounded_name: BoundedVec<u8, T::StringLimit> =
				name.clone().try_into().map_err(|_| Error::<T, I>::BadMetadata)?;

			let bounded_symbol: BoundedVec<u8, T::StringLimit> =
				symbol.clone().try_into().map_err(|_| Error::<T, I>::BadMetadata)?;

			ensure!(Asset::<T, I>::contains_key(id), Error::<T, I>::Unknown);
			Metadata::<T, I>::try_mutate_exists(id, |metadata| {
				let deposit = metadata.take().map_or(Zero::zero(), |m| m.deposit);
				*metadata = Some(AssetMetadata {
					deposit,
					name: bounded_name,
					symbol: bounded_symbol,
					decimals,
					is_frozen,
				});

				Self::deposit_event(Event::MetadataSet(id, name, symbol, decimals, is_frozen));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::force_clear_metadata())]
		pub fn force_clear_metadata(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;

			let d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			Metadata::<T, I>::try_mutate_exists(id, |metadata| {
				let deposit = metadata.take().ok_or(Error::<T, I>::Unknown)?.deposit;
				T::Currency::unreserve(&d.owner, deposit);
				Self::deposit_event(Event::MetadataCleared(id));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::force_asset_status())]
		pub fn force_asset_status(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			owner: <T::Lookup as StaticLookup>::Source,
			issuer: <T::Lookup as StaticLookup>::Source,
			admin: <T::Lookup as StaticLookup>::Source,
			freezer: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] min_balance: T::Balance,
			is_sufficient: bool,
			is_frozen: bool,
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;

			Asset::<T, I>::try_mutate(id, |maybe_asset| {
				let mut asset = maybe_asset.take().ok_or(Error::<T, I>::Unknown)?;
				asset.owner = T::Lookup::lookup(owner)?;
				asset.issuer = T::Lookup::lookup(issuer)?;
				asset.admin = T::Lookup::lookup(admin)?;
				asset.freezer = T::Lookup::lookup(freezer)?;
				asset.min_balance = min_balance;
				asset.is_sufficient = is_sufficient;
				asset.is_frozen = is_frozen;
				*maybe_asset = Some(asset);

				Self::deposit_event(Event::AssetStatusChanged(id));
				Ok(())
			})
		}

		#[pallet::weight(T::WeightInfo::approve_transfer())]
		pub fn approve_transfer(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			delegate: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let delegate = T::Lookup::lookup(delegate)?;

			let mut d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			ensure!(!d.is_frozen, Error::<T, I>::Frozen);
			Approvals::<T, I>::try_mutate(
				(id, &owner, &delegate),
				|maybe_approved| -> DispatchResult {
					let mut approved = match maybe_approved.take() {
						// an approval already exists and is being updated
						Some(a) => a,
						// a new approval is created
						None => {
							d.approvals.saturating_inc();
							Default::default()
						}
					};
					let deposit_required = T::ApprovalDeposit::get();
					if approved.deposit < deposit_required {
						T::Currency::reserve(&owner, deposit_required - approved.deposit)?;
						approved.deposit = deposit_required;
					}
					approved.amount = approved.amount.saturating_add(amount);
					*maybe_approved = Some(approved);
					Ok(())
				},
			)?;
			Asset::<T, I>::insert(id, d);
			Self::deposit_event(Event::ApprovedTransfer(id, owner, delegate, amount));

			Ok(())
		}

		#[pallet::weight(T::WeightInfo::cancel_approval())]
		pub fn cancel_approval(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			delegate: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let delegate = T::Lookup::lookup(delegate)?;
			let mut d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			let approval =
				Approvals::<T, I>::take((id, &owner, &delegate)).ok_or(Error::<T, I>::Unknown)?;
			T::Currency::unreserve(&owner, approval.deposit);

			d.approvals.saturating_dec();
			Asset::<T, I>::insert(id, d);

			Self::deposit_event(Event::ApprovalCancelled(id, owner, delegate));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::force_cancel_approval())]
		pub fn force_cancel_approval(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			owner: <T::Lookup as StaticLookup>::Source,
			delegate: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			let mut d = Asset::<T, I>::get(id).ok_or(Error::<T, I>::Unknown)?;
			T::ForceOrigin::try_origin(origin)
				.map(|_| ())
				.or_else(|origin| -> DispatchResult {
					let origin = ensure_signed(origin)?;
					ensure!(&origin == &d.admin, Error::<T, I>::NoPermission);
					Ok(())
				})?;

			let owner = T::Lookup::lookup(owner)?;
			let delegate = T::Lookup::lookup(delegate)?;

			let approval =
				Approvals::<T, I>::take((id, &owner, &delegate)).ok_or(Error::<T, I>::Unknown)?;
			T::Currency::unreserve(&owner, approval.deposit);
			d.approvals.saturating_dec();
			Asset::<T, I>::insert(id, d);

			Self::deposit_event(Event::ApprovalCancelled(id, owner, delegate));
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::transfer_approved())]
		pub fn transfer_approved(
			origin: OriginFor<T>,
			#[pallet::compact] id: T::AssetId,
			owner: <T::Lookup as StaticLookup>::Source,
			destination: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] amount: T::Balance,
		) -> DispatchResult {
			let delegate = ensure_signed(origin)?;
			let owner = T::Lookup::lookup(owner)?;
			let destination = T::Lookup::lookup(destination)?;

			Approvals::<T, I>::try_mutate_exists(
				(id, &owner, delegate),
				|maybe_approved| -> DispatchResult {
					let mut approved = maybe_approved.take().ok_or(Error::<T, I>::Unapproved)?;
					let remaining =
						approved.amount.checked_sub(&amount).ok_or(Error::<T, I>::Unapproved)?;

					let f =
						TransferFlags { keep_alive: false, best_effort: false, burn_dust: false };
					Self::do_transfer(id, &owner, &destination, amount, None, f)?;

					if remaining.is_zero() {
						T::Currency::unreserve(&owner, approved.deposit);
						Asset::<T, I>::mutate(id, |maybe_details| {
							if let Some(details) = maybe_details {
								details.approvals.saturating_dec();
							}
						});
					} else {
						approved.amount = remaining;
						*maybe_approved = Some(approved);
					}
					Ok(())
				},
			)?;
			Ok(())
		}
	}
}
