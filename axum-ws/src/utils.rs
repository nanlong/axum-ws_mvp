use crate::{BroadcastMessage, SENDER};

pub(crate) fn broadcast(message: BroadcastMessage) -> anyhow::Result<()> {
    SENDER.send(message)?;
    Ok(())
}
