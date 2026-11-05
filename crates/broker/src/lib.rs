//! Brokerage integration through [SnapTrade](https://snaptrade.com): a signed API client, a
//! normalised portfolio model (accounts, balances, historic orders, activity), secure
//! credential storage, and an offline sample portfolio. No order entry API is compiled.
//! Registration and explicit disconnect remain account-management operations.

pub mod model;
pub mod sample;
pub mod secrets;
pub mod snaptrade;

pub use model::{Account, Activity, Connection, InstrumentKind, Order, Portfolio, Position, Totals};
pub use secrets::{Credentials, SecretStore};
pub use snaptrade::SnapTrade;
