//! Synthetic TL responses shared by the explicit development executable/tests.
use tg_mtproto::{AccountIdentity, AccountKind, tl};
use tl::{Deserializable, Identifiable, Serializable};
pub fn state(pts: i32) -> tl::enums::updates::State {
    tl::types::updates::State {
        pts,
        qts: 0,
        seq: 0,
        date: 100 + pts,
        unread_count: 0,
    }
    .into()
}
pub fn user(identity: AccountIdentity) -> tl::enums::User {
    let bot = identity.kind == AccountKind::Bot;
    let mut v = tl::types::User::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&((1u32 << 10) | if bot { 1 << 14 } else { 0 }).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&identity.id.get().to_le_bytes());
    if bot {
        v.extend_from_slice(&0i32.to_le_bytes());
    }
    tl::enums::User::from_bytes(&v).unwrap()
}
pub fn is_write(bytes: &[u8]) -> bool {
    let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    [
        tl::functions::messages::SendMessage::CONSTRUCTOR_ID,
        tl::functions::messages::EditMessage::CONSTRUCTOR_ID,
        tl::functions::messages::SendMedia::CONSTRUCTOR_ID,
        tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID,
    ]
    .contains(&ctor)
}
fn sent(media: Option<tl::enums::MessageMedia>) -> Vec<u8> {
    tl::enums::Updates::UpdateShortSentMessage(tl::types::UpdateShortSentMessage {
        out: true,
        id: 1,
        pts: 1,
        pts_count: 1,
        date: 123,
        media,
        entities: None,
        ttl_period: None,
    })
    .to_bytes()
}
fn media() -> tl::enums::MessageMedia {
    let mut v = tl::types::MessageMediaDocument::CONSTRUCTOR_ID
        .to_le_bytes()
        .to_vec();
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&tl::types::Document::CONSTRUCTOR_ID.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&1i64.to_le_bytes());
    v.extend_from_slice(&777i64.to_le_bytes());
    v.extend_from_slice(&b"PRIVATE_FILE_REFERENCE".to_vec().to_bytes());
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&"application/octet-stream".to_owned().to_bytes());
    v.extend_from_slice(&3i64.to_le_bytes());
    v.extend_from_slice(&2i32.to_le_bytes());
    v.extend_from_slice(
        &vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "sample.bin".into(),
            },
        )]
        .to_bytes(),
    );
    tl::enums::MessageMedia::from_bytes(&v).unwrap()
}
pub fn response(identity: AccountIdentity, bytes: &[u8]) -> Vec<u8> {
    let ctor = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    if ctor == tl::functions::users::GetUsers::CONSTRUCTOR_ID {
        return vec![user(identity)].to_bytes();
    }
    if ctor == tl::functions::updates::GetState::CONSTRUCTOR_ID {
        return state(1).to_bytes();
    }
    if ctor == tl::functions::updates::GetDifference::CONSTRUCTOR_ID {
        return tl::enums::updates::Difference::Difference(tl::types::updates::Difference {
            new_messages: vec![],
            new_encrypted_messages: vec![],
            chats: vec![],
            users: vec![],
            other_updates: vec![
                tl::types::UpdateBotCallbackQuery {
                    query_id: 3,
                    user_id: 99,
                    peer: tl::types::PeerUser {
                        user_id: identity.id.get(),
                    }
                    .into(),
                    msg_id: 1,
                    chat_instance: 888,
                    data: Some(b"untrusted payload".to_vec()),
                    game_short_name: None,
                }
                .into(),
            ],
            state: state(2),
        })
        .to_bytes();
    }
    if ctor == tl::functions::upload::SaveFilePart::CONSTRUCTOR_ID {
        let part = tl::functions::upload::SaveFilePart::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(part.file_part, 0);
        assert_eq!(part.bytes, b"abc");
        return true.to_bytes();
    }
    if ctor == tl::functions::messages::SendMessage::CONSTRUCTOR_ID {
        return sent(None);
    }
    if ctor == tl::functions::messages::SendMedia::CONSTRUCTOR_ID {
        return sent(Some(media()));
    }
    if ctor == tl::functions::messages::SetBotCallbackAnswer::CONSTRUCTOR_ID {
        let answer =
            tl::functions::messages::SetBotCallbackAnswer::from_bytes(&bytes[4..]).unwrap();
        assert_eq!(answer.query_id, 3);
        return true.to_bytes();
    }
    assert_eq!(ctor, tl::functions::messages::EditMessage::CONSTRUCTOR_ID);
    let edit = tl::functions::messages::EditMessage::from_bytes(&bytes[4..]).unwrap();
    let mut v = tl::types::Message::CONSTRUCTOR_ID.to_le_bytes().to_vec();
    v.extend_from_slice(&2u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&edit.id.to_le_bytes());
    v.extend_from_slice(
        &tl::enums::Peer::User(tl::types::PeerUser {
            user_id: identity.id.get(),
        })
        .to_bytes(),
    );
    v.extend_from_slice(&123i32.to_le_bytes());
    v.extend_from_slice(&edit.message.unwrap().to_bytes());
    tl::enums::Updates::Updates(tl::types::Updates {
        updates: vec![
            tl::types::UpdateEditMessage {
                message: tl::enums::Message::from_bytes(&v).unwrap(),
                pts: 2,
                pts_count: 1,
            }
            .into(),
        ],
        users: vec![],
        chats: vec![],
        date: 123,
        seq: 2,
    })
    .to_bytes()
}
