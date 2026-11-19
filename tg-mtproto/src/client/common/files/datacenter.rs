use super::super::*;
use crate::client::error::RpcClass;
use zeroize::{Zeroize, Zeroizing};

struct Import(tl::functions::auth::ImportAuthorization);
impl Drop for Import {
    fn drop(&mut self) {
        self.0.bytes.zeroize();
    }
}
impl Context {
    pub(in crate::client) async fn verify_file_dc(
        &self,
        expected: AccountIdentity,
        dc: i32,
        gate: Arc<tokio::sync::Mutex<()>>,
        limit: usize,
    ) -> Result<()> {
        self.check_active()?;
        if dc == self.home_dc()? {
            return Ok(());
        }
        let guard = gate.lock_owned().await;
        self.check_active()?;
        let mut request = super::super::super::encode_request(
            &tl::functions::users::GetUsers {
                id: vec![tl::enums::InputUser::UserSelf],
            },
            limit,
        )?;
        match self
            .invoke_in_dc::<tl::functions::users::GetUsers>(dc, std::mem::take(&mut *request))
            .await
        {
            Ok(mut users) => {
                if users.len() != 1 {
                    return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
                }
                auth::checked_peer(users.remove(0), expected)?;
                return Ok(());
            }
            Err(failure) if failure.rpc_class() == RpcClass::Unregistered => {}
            Err(failure) => return Err(failure),
        }
        let mut request = super::super::super::encode_request(
            &tl::functions::auth::ExportAuthorization { dc_id: dc },
            limit,
        )?;
        let tl::enums::auth::ExportedAuthorization::Authorization(exported) = self
            .invoke::<tl::functions::auth::ExportAuthorization>(std::mem::take(&mut *request))
            .await?;
        let mut ticket = Zeroizing::new(exported.bytes);
        if exported.id != expected.id.get() || ticket.is_empty() || ticket.len() > 16 * 1024 {
            return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
        }
        let mut body = {
            let request = Import(tl::functions::auth::ImportAuthorization {
                id: exported.id,
                bytes: std::mem::take(&mut *ticket),
            });
            super::super::super::encode_request(&request.0, limit)?
        };
        self.begin_dc_authorization(guard)?;
        let response = self
            .invoke_in_dc::<tl::functions::auth::ImportAuthorization>(
                dc,
                std::mem::take(&mut *body),
            )
            .await?;
        let tl::enums::auth::Authorization::Authorization(mut authorization) = response else {
            return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
        };
        if let Some(token) = &mut authorization.future_auth_token {
            token.zeroize();
        }
        auth::checked_peer(authorization.user, expected)?;
        self.confirm_dc_authorization()
    }
}
