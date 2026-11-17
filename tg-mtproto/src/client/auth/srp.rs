use super::{ClientErrorKind, Outcome, Result, error, tl};
use num_bigint::BigUint;
use zeroize::{Zeroize, Zeroizing};

pub(super) struct Parameters {
    salt1: Zeroizing<Vec<u8>>,
    salt2: Zeroizing<Vec<u8>>,
    p: Zeroizing<Vec<u8>>,
    g: i32,
    b: Zeroizing<Vec<u8>>,
    pub id: i64,
}
pub(super) struct Proof {
    pub a: Zeroizing<[u8; 256]>,
    pub m1: Zeroizing<[u8; 32]>,
}
pub(super) struct PasswordInfo {
    pub parameters: Parameters,
    pub hint: Option<Zeroizing<String>>,
}
struct RawPassword(tl::types::account::Password);
fn wipe_algorithm(algo: &mut tl::enums::PasswordKdfAlgo) {
    if let tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(algo) =
        algo
    {
        algo.salt1.zeroize();
        algo.salt2.zeroize();
        algo.p.zeroize();
    }
}
impl Drop for RawPassword {
    fn drop(&mut self) {
        self.0.secure_random.zeroize();
        self.0.srp_b.zeroize();
        self.0.hint.zeroize();
        self.0.email_unconfirmed_pattern.zeroize();
        self.0.login_email_pattern.zeroize();
        if let Some(algo) = self.0.current_algo.as_mut() {
            wipe_algorithm(algo);
        }
        wipe_algorithm(&mut self.0.new_algo);
    }
}
fn invalid() -> super::super::ClientError {
    error(ClientErrorKind::Decode, Outcome::Unknown)
}
pub(super) fn parameters(info: tl::types::account::Password) -> Result<Option<PasswordInfo>> {
    let mut raw = RawPassword(info);
    if !raw.0.has_password {
        return Err(invalid());
    }
    let algo = raw.0.current_algo.take().ok_or_else(invalid)?;
    let tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(mut algo) =
        algo
    else {
        return Ok(None);
    };
    let parameters = Parameters {
        salt1: Zeroizing::new(std::mem::take(&mut algo.salt1)),
        salt2: Zeroizing::new(std::mem::take(&mut algo.salt2)),
        p: Zeroizing::new(std::mem::take(&mut algo.p)),
        g: algo.g,
        b: Zeroizing::new(raw.0.srp_b.take().ok_or_else(invalid)?),
        id: raw.0.srp_id.take().ok_or_else(invalid)?,
    };
    if parameters.p.len() != 256
        || parameters.p[0] & 0x80 == 0
        || !(2..=7).contains(&parameters.g)
        || parameters.salt1.is_empty()
        || parameters.salt1.len() > 1024
        || parameters.salt2.is_empty()
        || parameters.salt2.len() > 1024
        || !(248..=256).contains(&parameters.b.len())
    {
        return Err(invalid());
    }
    let b = BigUint::from_bytes_be(&parameters.b);
    if b.bits() < 1984 || b >= BigUint::from_bytes_be(&parameters.p) {
        return Err(invalid());
    }
    let hint = raw.0.hint.take().map(Zeroizing::new);
    if hint
        .as_ref()
        .is_some_and(|hint| hint.len() > 512 || hint.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    Ok(Some(PasswordInfo { parameters, hint }))
}
pub(super) fn proof(parameters: &Parameters, password: &[u8]) -> Result<Proof> {
    if !grammers_crypto::two_factor_auth::check_p_and_g(&parameters.p, &parameters.g) {
        return Err(invalid());
    }
    let mut exponent = Zeroizing::new(vec![0u8; 256]);
    getrandom::fill(&mut exponent).map_err(|_| invalid())?;
    let (mut m1, mut a) = grammers_crypto::two_factor_auth::calculate_2fa(
        &parameters.salt1,
        &parameters.salt2,
        &parameters.p,
        &parameters.g,
        parameters.b.to_vec(),
        exponent.to_vec(),
        password,
    );
    let result = Proof {
        a: Zeroizing::new(a),
        m1: Zeroizing::new(m1),
    };
    a.zeroize();
    m1.zeroize();
    let public_a = BigUint::from_bytes_be(result.a.as_ref());
    let mut b_padded = [0u8; 256];
    b_padded[256 - parameters.b.len()..].copy_from_slice(&parameters.b);
    let u = grammers_crypto::sha256!(result.a.as_ref(), &b_padded);
    if public_a.bits() < 1984
        || public_a >= BigUint::from_bytes_be(&parameters.p)
        || u.iter().all(|byte| *byte == 0)
    {
        return Err(invalid());
    }
    Ok(result)
}
