#![cfg_attr(not(feature = "std"), no_std)]

// mod extra_mutator;
pub mod weights;
pub use extra_mutator::*;
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

mod functions {
	use super::*;

	// The main implementation block for the module.
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		// Public immutables

		/// Return the extra "sid-car" data for `id`/`who`, or `None` if the account doesn't exist.
		pub fn adjust_extra(
			id: T::AssetId,
			who: impl sp_std::borrow::Borrow<T::AccountId>,
		) -> Option<ExtraMutator<T, I>> {
			ExtraMutator::maybe_new(id, who)
		}

		/// Get the asset `id` balance of `who`.
		pub fn balance(
			id: T::AssetId,
			who: impl sp_std::borrow::Borrow<T::AccountId>,
		) -> T::Balance {
			Account::<T, I>::get(id, who.borrow()).balance
		}

		/// Get the total supply of an asset `id`.
		pub fn total_supply(id: T::AssetId) -> T::Balance {
			Asset::<T, I>::get(id).map(|x| x.supply).unwrap_or_else(Zero::zero)
		}

		pub(super) fn new_account(
			who: &T::AccountId,
			d: &mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
		) -> Result<bool, DispatchError> {
			let accounts = d.accounts.checked_add(1).ok_or(ArithmeticError::Overflow)?;
			let is_sufficient = if d.is_sufficient {
				frame_system::Pallet::<T>::inc_sufficients(who);
				d.sufficients += 1;
				true
			} else {
				frame_system::Pallet::<T>::inc_consumers(who)
					.map_err(|_| Error::<T, I>::NoProvider)?;
				false
			};
			d.accounts = accounts;
			Ok(is_sufficient)
		}

		pub(super) fn dead_account(
			what: T::AssetId,
			who: &T::AccountId,
			d: &mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
			sufficient: bool,
		) {
			if sufficient {
				d.sufficients = d.sufficients.saturating_sub(1);
				frame_system::Pallet::<T>::dec_sufficients(who);
			} else {
				frame_system::Pallet::<T>::dec_consumers(who);
			}
			d.accounts = d.accounts.saturating_sub(1);
			T::Freezer::died(what, who)
		}

		pub(super) fn can_increase(
			id: T::AssetId,
			who: &T::AccountId,
			amount: T::Balance,
		) -> DepositConsequence {
			let details = match Asset::<T, I>::get(id) {
				Some(details) => details,
				None => return DepositConsequence::UnknownAsset,
			};
			if details.supply.checked_add(&amount).is_none() {
				return DepositConsequence::Overflow;
			}
			let account = Account::<T, I>::get(id, who);
			if account.balance.checked_add(&amount).is_none() {
				return DepositConsequence::Overflow;
			}
			if account.balance.is_zero() {
				if amount < details.min_balance {
					return DepositConsequence::BelowMinimum;
				}
				if !details.is_sufficient && frame_system::Pallet::<T>::providers(who) == 0 {
					return DepositConsequence::CannotCreate;
				}
				if details.is_sufficient && details.sufficients.checked_add(1).is_none() {
					return DepositConsequence::Overflow;
				}
			}

			DepositConsequence::Success
		}

		/// Return the consequence of a withdraw.
		pub(super) fn can_decrease(
			id: T::AssetId,
			who: &T::AccountId,
			amount: T::Balance,
			keep_alive: bool,
		) -> WithdrawConsequence<T::Balance> {
			use WithdrawConsequence::*;
			let details = match Asset::<T, I>::get(id) {
				Some(details) => details,
				None => return UnknownAsset,
			};
			if details.supply.checked_sub(&amount).is_none() {
				return Underflow;
			}
			if details.is_frozen {
				return Frozen;
			}
			let account = Account::<T, I>::get(id, who);
			if account.is_frozen {
				return Frozen;
			}
			if let Some(rest) = account.balance.checked_sub(&amount) {
				if let Some(frozen) = T::Freezer::frozen_balance(id, who) {
					match frozen.checked_add(&details.min_balance) {
						Some(required) if rest < required => return Frozen,
						None => return Overflow,
						_ => {}
					}
				}

				let is_provider = false;
				let is_required = is_provider && !frame_system::Pallet::<T>::can_dec_provider(who);
				let must_keep_alive = keep_alive || is_required;

				if rest < details.min_balance {
					if must_keep_alive {
						WouldDie
					} else {
						ReducedToZero(rest)
					}
				} else {
					Success
				}
			} else {
				NoFunds
			}
		}

		// Maximum `amount` that can be passed into `can_withdraw` to result in a `WithdrawConsequence`
		// of `Success`.
		pub(super) fn reducible_balance(
			id: T::AssetId,
			who: &T::AccountId,
			keep_alive: bool,
		) -> Result<T::Balance, DispatchError> {
			let details = Asset::<T, I>::get(id).ok_or_else(|| Error::<T, I>::Unknown)?;
			ensure!(!details.is_frozen, Error::<T, I>::Frozen);

			let account = Account::<T, I>::get(id, who);
			ensure!(!account.is_frozen, Error::<T, I>::Frozen);

			let amount = if let Some(frozen) = T::Freezer::frozen_balance(id, who) {
				// Frozen balance: account CANNOT be deleted
				let required =
					frozen.checked_add(&details.min_balance).ok_or(ArithmeticError::Overflow)?;
				account.balance.saturating_sub(required)
			} else {
				let is_provider = false;
				let is_required = is_provider && !frame_system::Pallet::<T>::can_dec_provider(who);
				if keep_alive || is_required {
					// We want to keep the account around.
					account.balance.saturating_sub(details.min_balance)
				} else {
					// Don't care if the account dies
					account.balance
				}
			};
			Ok(amount.min(details.supply))
		}

		pub(super) fn prep_debit(
			id: T::AssetId,
			target: &T::AccountId,
			amount: T::Balance,
			f: DebitFlags,
		) -> Result<T::Balance, DispatchError> {
			let actual = Self::reducible_balance(id, target, f.keep_alive)?.min(amount);
			ensure!(f.best_effort || actual >= amount, Error::<T, I>::BalanceLow);

			let conseq = Self::can_decrease(id, target, actual, f.keep_alive);
			let actual = match conseq.into_result() {
				Ok(dust) => actual.saturating_add(dust), //< guaranteed by reducible_balance
				Err(e) => {
					debug_assert!(false, "passed from reducible_balance; qed");
					return Err(e.into());
				}
			};

			Ok(actual)
		}

		pub(super) fn prep_credit(
			id: T::AssetId,
			dest: &T::AccountId,
			amount: T::Balance,
			debit: T::Balance,
			burn_dust: bool,
		) -> Result<(T::Balance, Option<T::Balance>), DispatchError> {
			let (credit, maybe_burn) = match (burn_dust, debit.checked_sub(&amount)) {
				(true, Some(dust)) => (amount, Some(dust)),
				_ => (debit, None),
			};
			Self::can_increase(id, &dest, credit).into_result()?;
			Ok((credit, maybe_burn))
		}

		pub(super) fn do_mint(
			id: T::AssetId,
			beneficiary: &T::AccountId,
			amount: T::Balance,
			maybe_check_issuer: Option<T::AccountId>,
		) -> DispatchResult {
			Self::increase_balance(id, beneficiary, amount, |details| -> DispatchResult {
				if let Some(check_issuer) = maybe_check_issuer {
					ensure!(&check_issuer == &details.issuer, Error::<T, I>::NoPermission);
				}
				debug_assert!(
					T::Balance::max_value() - details.supply >= amount,
					"checked in prep; qed"
				);
				details.supply = details.supply.saturating_add(amount);
				Ok(())
			})?;
			Self::deposit_event(Event::Issued(id, beneficiary.clone(), amount));
			Ok(())
		}

		pub(super) fn increase_balance(
			id: T::AssetId,
			beneficiary: &T::AccountId,
			amount: T::Balance,
			check: impl FnOnce(
				&mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
			) -> DispatchResult,
		) -> DispatchResult {
			if amount.is_zero() {
				return Ok(());
			}

			Self::can_increase(id, beneficiary, amount).into_result()?;
			Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
				let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

				check(details)?;

				Account::<T, I>::try_mutate(id, beneficiary, |t| -> DispatchResult {
					let new_balance = t.balance.saturating_add(amount);
					ensure!(new_balance >= details.min_balance, TokenError::BelowMinimum);
					if t.balance.is_zero() {
						t.sufficient = Self::new_account(beneficiary, details)?;
					}
					t.balance = new_balance;
					Ok(())
				})?;
				Ok(())
			})?;
			Ok(())
		}

		pub(super) fn do_burn(
			id: T::AssetId,
			target: &T::AccountId,
			amount: T::Balance,
			maybe_check_admin: Option<T::AccountId>,
			f: DebitFlags,
		) -> Result<T::Balance, DispatchError> {
			let actual = Self::decrease_balance(id, target, amount, f, |actual, details| {
				// Check admin rights.
				if let Some(check_admin) = maybe_check_admin {
					ensure!(&check_admin == &details.admin, Error::<T, I>::NoPermission);
				}

				debug_assert!(details.supply >= actual, "checked in prep; qed");
				details.supply = details.supply.saturating_sub(actual);

				Ok(())
			})?;
			Self::deposit_event(Event::Burned(id, target.clone(), actual));
			Ok(actual)
		}

		pub(super) fn decrease_balance(
			id: T::AssetId,
			target: &T::AccountId,
			amount: T::Balance,
			f: DebitFlags,
			check: impl FnOnce(
				T::Balance,
				&mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
			) -> DispatchResult,
		) -> Result<T::Balance, DispatchError> {
			if amount.is_zero() {
				return Ok(amount);
			}

			let actual = Self::prep_debit(id, target, amount, f)?;

			Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
				let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

				check(actual, details)?;

				Account::<T, I>::try_mutate_exists(
					id,
					target,
					|maybe_account| -> DispatchResult {
						let mut account = maybe_account.take().unwrap_or_default();
						debug_assert!(account.balance >= actual, "checked in prep; qed");

						// Make the debit.
						account.balance = account.balance.saturating_sub(actual);
						*maybe_account = if account.balance < details.min_balance {
							debug_assert!(account.balance.is_zero(), "checked in prep; qed");
							Self::dead_account(id, target, details, account.sufficient);
							None
						} else {
							Some(account)
						};
						Ok(())
					},
				)?;

				Ok(())
			})?;

			Ok(actual)
		}

		pub(super) fn do_transfer(
			id: T::AssetId,
			source: &T::AccountId,
			dest: &T::AccountId,
			amount: T::Balance,
			maybe_need_admin: Option<T::AccountId>,
			f: TransferFlags,
		) -> Result<T::Balance, DispatchError> {
			// Early exist if no-op.
			if amount.is_zero() {
				Self::deposit_event(Event::Transferred(id, source.clone(), dest.clone(), amount));
				return Ok(amount);
			}

			// Figure out the debit and credit, together with side-effects.
			let debit = Self::prep_debit(id, &source, amount, f.into())?;
			let (credit, maybe_burn) = Self::prep_credit(id, &dest, amount, debit, f.burn_dust)?;

			let mut source_account = Account::<T, I>::get(id, &source);

			Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
				let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

				// Check admin rights.
				if let Some(need_admin) = maybe_need_admin {
					ensure!(&need_admin == &details.admin, Error::<T, I>::NoPermission);
				}

				// Skip if source == dest
				if source == dest {
					return Ok(());
				}

				// Burn any dust if needed.
				if let Some(burn) = maybe_burn {
					// Debit dust from supply; this will not saturate since it's already checked in
					// prep.
					debug_assert!(details.supply >= burn, "checked in prep; qed");
					details.supply = details.supply.saturating_sub(burn);
				}

				// Debit balance from source; this will not saturate since it's already checked in prep.
				debug_assert!(source_account.balance >= debit, "checked in prep; qed");
				source_account.balance = source_account.balance.saturating_sub(debit);

				Account::<T, I>::try_mutate(id, &dest, |a| -> DispatchResult {
					// Calculate new balance; this will not saturate since it's already checked in prep.
					debug_assert!(a.balance.checked_add(&credit).is_some(), "checked in prep; qed");
					let new_balance = a.balance.saturating_add(credit);

					// Create a new account if there wasn't one already.
					if a.balance.is_zero() {
						a.sufficient = Self::new_account(&dest, details)?;
					}

					a.balance = new_balance;
					Ok(())
				})?;

				// Remove source account if it's now dead.
				if source_account.balance < details.min_balance {
					debug_assert!(source_account.balance.is_zero(), "checked in prep; qed");
					Self::dead_account(id, &source, details, source_account.sufficient);
					Account::<T, I>::remove(id, &source);
				} else {
					Account::<T, I>::insert(id, &source, &source_account)
				}

				Ok(())
			})?;

			Self::deposit_event(Event::Transferred(id, source.clone(), dest.clone(), credit));
			Ok(credit)
		}
	}
}

mod types {
	use super::*;
	use frame_support::pallet_prelude::*;

	use frame_support::traits::{fungible, tokens::BalanceConversion};
	use sp_runtime::{traits::Convert, FixedPointNumber, FixedPointOperand, FixedU128};

	pub(super) type DepositBalanceOf<T, I = ()> =
		<<T as Config<I>>::Currency as Currency<<T as SystemConfig>::AccountId>>::Balance;

	#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen)]
	pub struct AssetDetails<Balance, AccountId, DepositBalance> {
		/// Can change `owner`, `issuer`, `freezer` and `admin` accounts.
		pub(super) owner: AccountId,
		/// Can mint tokens.
		pub(super) issuer: AccountId,
		/// Can thaw tokens, force transfers and burn tokens from any account.
		pub(super) admin: AccountId,
		/// Can freeze tokens.
		pub(super) freezer: AccountId,
		/// The total supply across all accounts.
		pub(super) supply: Balance,
		/// The balance deposited for this asset. This pays for the data stored here.
		pub(super) deposit: DepositBalance,
		/// The ED for virtual accounts.
		pub(super) min_balance: Balance,
		/// If `true`, then any account with this asset is given a provider reference. Otherwise, it
		/// requires a consumer reference.
		pub(super) is_sufficient: bool,
		/// The total number of accounts.
		pub(super) accounts: u32,
		/// The total number of accounts for which we have placed a self-sufficient reference.
		pub(super) sufficients: u32,
		/// The total number of approvals.
		pub(super) approvals: u32,
		/// Whether the asset is frozen for non-admin transfers.
		pub(super) is_frozen: bool,
	}

	impl<Balance, AccountId, DepositBalance> AssetDetails<Balance, AccountId, DepositBalance> {
		pub fn destroy_witness(&self) -> DestroyWitness {
			DestroyWitness {
				accounts: self.accounts,
				sufficients: self.sufficients,
				approvals: self.approvals,
			}
		}
	}

	/// Data concerning an approval.
	#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, MaxEncodedLen)]
	pub struct Approval<Balance, DepositBalance> {
		/// The amount of funds approved for the balance transfer from the owner to some delegated
		/// target.
		pub(super) amount: Balance,
		/// The amount reserved on the owner's account to hold this item in storage.
		pub(super) deposit: DepositBalance,
	}

	#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, MaxEncodedLen)]
	pub struct AssetBalance<Balance, Extra> {
		/// The balance.
		pub(super) balance: Balance,
		/// Whether the account is frozen.
		pub(super) is_frozen: bool,
		/// `true` if this balance gave the account a self-sufficient reference.
		pub(super) sufficient: bool,
		/// Additional "sidecar" data, in case some other pallet wants to use this storage item.
		pub(super) extra: Extra,
	}

	#[derive(Clone, Encode, Decode, Eq, PartialEq, Default, RuntimeDebug, MaxEncodedLen)]
	pub struct AssetMetadata<DepositBalance, BoundedString> {
		/// The balance deposited for this metadata.
		///
		/// This pays for the data stored in this struct.
		pub(super) deposit: DepositBalance,
		/// The user friendly name of this asset. Limited in length by `StringLimit`.
		pub(super) name: BoundedString,
		/// The ticker symbol for this asset. Limited in length by `StringLimit`.
		pub(super) symbol: BoundedString,
		/// The number of decimals this asset uses to represent one unit.
		pub(super) decimals: u8,
		/// Whether the asset metadata may be changed by a non Force origin.
		pub(super) is_frozen: bool,
	}

	/// Witness data for the destroy transactions.
	#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen)]
	pub struct DestroyWitness {
		/// The number of accounts holding the asset.
		#[codec(compact)]
		pub(super) accounts: u32,
		/// The number of accounts holding the asset with a self-sufficient reference.
		#[codec(compact)]
		pub(super) sufficients: u32,
		/// The number of transfer-approvals of the asset.
		#[codec(compact)]
		pub(super) approvals: u32,
	}

	pub trait FrozenBalance<AssetId, AccountId, Balance> {
		fn frozen_balance(asset: AssetId, who: &AccountId) -> Option<Balance>;

		/// Called when an account has been removed.
		fn died(asset: AssetId, who: &AccountId);
	}

	impl<AssetId, AccountId, Balance> FrozenBalance<AssetId, AccountId, Balance> for () {
		fn frozen_balance(_: AssetId, _: &AccountId) -> Option<Balance> {
			None
		}
		fn died(_: AssetId, _: &AccountId) {}
	}

	#[derive(Copy, Clone, PartialEq, Eq)]
	pub(super) struct TransferFlags {
		pub(super) keep_alive: bool,
		pub(super) best_effort: bool,
		pub(super) burn_dust: bool,
	}

	#[derive(Copy, Clone, PartialEq, Eq)]
	pub(super) struct DebitFlags {
		pub(super) keep_alive: bool,
		pub(super) best_effort: bool,
	}

	impl From<TransferFlags> for DebitFlags {
		fn from(f: TransferFlags) -> Self {
			Self { keep_alive: f.keep_alive, best_effort: f.best_effort }
		}
	}

	/// Possible errors when converting between external and asset balances.
	#[derive(Eq, PartialEq, Copy, Clone, RuntimeDebug, Encode, Decode)]
	pub enum ConversionError {
		/// The external minimum balance must not be zero.
		MinBalanceZero,
		/// The asset is not present in storage.
		AssetMissing,
		/// The asset is not sufficient and thus does not have a reliable `min_balance` so it cannot be
		/// converted.
		AssetNotSufficient,
	}

	// Type alias for `frame_system`'s account id.
	type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
	// This pallet's asset id and balance type.
	type AssetIdOf<T, I> = <T as Config<I>>::AssetId;
	type AssetBalanceOf<T, I> = <T as Config<I>>::Balance;
	// Generic fungible balance type.
	type BalanceOf<F, T> = <F as fungible::Inspect<AccountIdOf<T>>>::Balance;

	pub struct BalanceToAssetBalance<F, T, CON, I = ()>(PhantomData<(F, T, CON, I)>);
	impl<F, T, CON, I> BalanceConversion<BalanceOf<F, T>, AssetIdOf<T, I>, AssetBalanceOf<T, I>>
		for BalanceToAssetBalance<F, T, CON, I>
	where
		F: fungible::Inspect<AccountIdOf<T>>,
		T: Config<I>,
		I: 'static,
		CON: Convert<BalanceOf<F, T>, AssetBalanceOf<T, I>>,
		BalanceOf<F, T>: FixedPointOperand + Zero,
		AssetBalanceOf<T, I>: FixedPointOperand + Zero,
	{
		type Error = ConversionError;

		fn to_asset_balance(
			balance: BalanceOf<F, T>,
			asset_id: AssetIdOf<T, I>,
		) -> Result<AssetBalanceOf<T, I>, ConversionError> {
			let asset = Asset::<T, I>::get(asset_id).ok_or(ConversionError::AssetMissing)?;
			// only sufficient assets have a min balance with reliable value
			ensure!(asset.is_sufficient, ConversionError::AssetNotSufficient);
			let min_balance = CON::convert(F::minimum_balance());
			// make sure we don't divide by zero
			ensure!(!min_balance.is_zero(), ConversionError::MinBalanceZero);
			let balance = CON::convert(balance);
			// balance * asset.min_balance / min_balance
			Ok(FixedU128::saturating_from_rational(asset.min_balance, min_balance)
				.saturating_mul_int(balance))
		}
	}
}

mod impl_fungibles {

	use super::*;

	impl<T: Config<I>, I: 'static> fungibles::Inspect<<T as SystemConfig>::AccountId> for Pallet<T, I> {
		type AssetId = T::AssetId;
		type Balance = T::Balance;

		fn total_issuance(asset: Self::AssetId) -> Self::Balance {
			Asset::<T, I>::get(asset).map(|x| x.supply).unwrap_or_else(Zero::zero)
		}

		fn minimum_balance(asset: Self::AssetId) -> Self::Balance {
			Asset::<T, I>::get(asset).map(|x| x.min_balance).unwrap_or_else(Zero::zero)
		}

		fn balance(asset: Self::AssetId, who: &<T as SystemConfig>::AccountId) -> Self::Balance {
			Pallet::<T, I>::balance(asset, who)
		}

		fn reducible_balance(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			keep_alive: bool,
		) -> Self::Balance {
			Pallet::<T, I>::reducible_balance(asset, who, keep_alive).unwrap_or(Zero::zero())
		}

		fn can_deposit(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			amount: Self::Balance,
		) -> DepositConsequence {
			Pallet::<T, I>::can_increase(asset, who, amount)
		}

		fn can_withdraw(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			amount: Self::Balance,
		) -> WithdrawConsequence<Self::Balance> {
			Pallet::<T, I>::can_decrease(asset, who, amount, false)
		}
	}

	impl<T: Config<I>, I: 'static> fungibles::Mutate<<T as SystemConfig>::AccountId> for Pallet<T, I> {
		fn mint_into(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			amount: Self::Balance,
		) -> DispatchResult {
			Self::do_mint(asset, who, amount, None)
		}

		fn burn_from(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			amount: Self::Balance,
		) -> Result<Self::Balance, DispatchError> {
			let f = DebitFlags { keep_alive: false, best_effort: false };
			Self::do_burn(asset, who, amount, None, f)
		}

		fn slash(
			asset: Self::AssetId,
			who: &<T as SystemConfig>::AccountId,
			amount: Self::Balance,
		) -> Result<Self::Balance, DispatchError> {
			let f = DebitFlags { keep_alive: false, best_effort: true };
			Self::do_burn(asset, who, amount, None, f)
		}
	}

	impl<T: Config<I>, I: 'static> fungibles::Transfer<T::AccountId> for Pallet<T, I> {
		fn transfer(
			asset: Self::AssetId,
			source: &T::AccountId,
			dest: &T::AccountId,
			amount: T::Balance,
			keep_alive: bool,
		) -> Result<T::Balance, DispatchError> {
			let f = TransferFlags { keep_alive, best_effort: false, burn_dust: false };
			Self::do_transfer(asset, source, dest, amount, None, f)
		}
	}

	impl<T: Config<I>, I: 'static> fungibles::Unbalanced<T::AccountId> for Pallet<T, I> {
		fn set_balance(_: Self::AssetId, _: &T::AccountId, _: Self::Balance) -> DispatchResult {
			unreachable!("set_balance is not used if other functions are impl'd");
		}
		fn set_total_issuance(id: T::AssetId, amount: Self::Balance) {
			Asset::<T, I>::mutate_exists(id, |maybe_asset| {
				if let Some(ref mut asset) = maybe_asset {
					asset.supply = amount
				}
			});
		}
		fn decrease_balance(
			asset: T::AssetId,
			who: &T::AccountId,
			amount: Self::Balance,
		) -> Result<Self::Balance, DispatchError> {
			let f = DebitFlags { keep_alive: false, best_effort: false };
			Self::decrease_balance(asset, who, amount, f, |_, _| Ok(()))
		}
		fn decrease_balance_at_most(
			asset: T::AssetId,
			who: &T::AccountId,
			amount: Self::Balance,
		) -> Self::Balance {
			let f = DebitFlags { keep_alive: false, best_effort: true };
			Self::decrease_balance(asset, who, amount, f, |_, _| Ok(())).unwrap_or(Zero::zero())
		}
		fn increase_balance(
			asset: T::AssetId,
			who: &T::AccountId,
			amount: Self::Balance,
		) -> Result<Self::Balance, DispatchError> {
			Self::increase_balance(asset, who, amount, |_| Ok(()))?;
			Ok(amount)
		}
		fn increase_balance_at_most(
			asset: T::AssetId,
			who: &T::AccountId,
			amount: Self::Balance,
		) -> Self::Balance {
			match Self::increase_balance(asset, who, amount, |_| Ok(())) {
				Ok(()) => amount,
				Err(_) => Zero::zero(),
			}
		}
	}
}

mod impl_stored_map {

	use super::*;

	impl<T: Config<I>, I: 'static> StoredMap<(T::AssetId, T::AccountId), T::Extra> for Pallet<T, I> {
		fn get(id_who: &(T::AssetId, T::AccountId)) -> T::Extra {
			let &(id, ref who) = id_who;
			if Account::<T, I>::contains_key(id, who) {
				Account::<T, I>::get(id, who).extra
			} else {
				Default::default()
			}
		}

		fn try_mutate_exists<R, E: From<DispatchError>>(
			id_who: &(T::AssetId, T::AccountId),
			f: impl FnOnce(&mut Option<T::Extra>) -> Result<R, E>,
		) -> Result<R, E> {
			let &(id, ref who) = id_who;
			let mut maybe_extra = Some(Account::<T, I>::get(id, who).extra);
			let r = f(&mut maybe_extra)?;
			Account::<T, I>::try_mutate_exists(id, who, |maybe_account| {
				if let Some(extra) = maybe_extra {
					// They want to write a value. Let this happen only if the account actually exists.
					if let Some(ref mut account) = maybe_account {
						account.extra = extra;
					} else {
						Err(DispatchError::NoProviders)?;
					}
				} else {
					// They want to delete it. Let this pass if the item never existed anyway.
					ensure!(maybe_account.is_none(), DispatchError::ConsumerRemaining);
				}
				Ok(r)
			})
		}
	}
}

mod extra_mutator {
	use super::*;

	pub struct ExtraMutator<T: Config<I>, I: 'static = ()> {
		id: T::AssetId,
		who: T::AccountId,
		original: T::Extra,
		pending: Option<T::Extra>,
	}

	impl<T: Config<I>, I: 'static> Drop for ExtraMutator<T, I> {
		fn drop(&mut self) {
			debug_assert!(self.commit().is_ok(), "attempt to write to non-existent asset account");
		}
	}

	impl<T: Config<I>, I: 'static> sp_std::ops::Deref for ExtraMutator<T, I> {
		type Target = T::Extra;
		fn deref(&self) -> &T::Extra {
			match self.pending {
				Some(ref value) => value,
				None => &self.original,
			}
		}
	}

	impl<T: Config<I>, I: 'static> sp_std::ops::DerefMut for ExtraMutator<T, I> {
		fn deref_mut(&mut self) -> &mut T::Extra {
			if self.pending.is_none() {
				self.pending = Some(self.original.clone());
			}
			self.pending.as_mut().unwrap()
		}
	}

	impl<T: Config<I>, I: 'static> ExtraMutator<T, I> {
		pub(super) fn maybe_new(
			id: T::AssetId,
			who: impl sp_std::borrow::Borrow<T::AccountId>,
		) -> Option<ExtraMutator<T, I>> {
			if Account::<T, I>::contains_key(id, who.borrow()) {
				Some(ExtraMutator::<T, I> {
					id,
					who: who.borrow().clone(),
					original: Account::<T, I>::get(id, who.borrow()).extra,
					pending: None,
				})
			} else {
				None
			}
		}

		/// Commit any changes to storage.
		pub fn commit(&mut self) -> Result<(), ()> {
			if let Some(extra) = self.pending.take() {
				Account::<T, I>::try_mutate_exists(self.id, self.who.borrow(), |maybe_account| {
					if let Some(ref mut account) = maybe_account {
						account.extra = extra;
						Ok(())
					} else {
						Err(())
					}
				})
			} else {
				Ok(())
			}
		}

		/// Revert any changes, even those already committed by `self` and drop self.
		pub fn revert(mut self) -> Result<(), ()> {
			self.pending = None;
			Account::<T, I>::try_mutate_exists(self.id, self.who.borrow(), |maybe_account| {
				if let Some(ref mut account) = maybe_account {
					account.extra = self.original.clone();
					Ok(())
				} else {
					Err(())
				}
			})
		}
	}
}
