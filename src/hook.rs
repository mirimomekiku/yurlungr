//! `yurlungr hook <agent> <state>`: called by an agent's own hook system to tell
//! the session server what that agent is doing. It must be quick, silent and
//! never fail, so a broken or missing server cannot disturb the agent.

use std::{env, time::Duration};

use crate::proto::{self, ClientMsg, ReportState};

pub fn report(agent: &str, state: ReportState) {
    let (Ok(pane), Ok(session)) = (env::var("YURLUNGR_PANE_ID"), env::var("YURLUNGR_SESSION"))
    else {
        // Not running inside a yurlungr pane.
        return;
    };
    let Ok(pane) = pane.parse::<usize>() else { return };
    let Some((mut stream, token)) = proto::connect(&session) else { return };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = proto::send(
        &mut stream,
        &ClientMsg::Report { token, pane, agent: agent.to_string(), state },
    );
}
