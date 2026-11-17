//! The `@` suggestions checking that the server members they list are still
//! members ([`discord::member_check`]): what they list goes to the check
//! once it has settled for half a second (462446), the check's requests go
//! out on the gateway, and whom an answer says left is forgotten, as the
//! client's `GUILD_MEMBER_REMOVE` forgets them.

use std::rc::Rc;
use std::time::Instant;

use discord::Snowflake;
use discord::member_check::SUGGESTIONS_WAIT;
use gtk::glib;

use super::App;

impl App {
    /// The suggestions list `users`. In a server, once the list has stayed
    /// the same for [`SUGGESTIONS_WAIT`], they are checked; the same list
    /// again doesn't restart the wait.
    pub(super) fn suggestions_listed(self: &Rc<Self>, users: Vec<Snowflake>) {
        let generation = {
            let mut state = self.account.state.borrow_mut();
            let Some(guild) = state.guild else { return self.drop_suggested(&mut state) };
            if state.suggested.as_ref().is_some_and(|(g, listed)| *g == guild && *listed == users) {
                return;
            }
            state.suggested = Some((guild, users));
            state.suggested_timer += 1;
            state.suggested_timer
        };
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(SUGGESTIONS_WAIT, move || {
            let Some(this) = this.upgrade().filter(|this| this.account.epoch.get() == epoch) else { return };
            {
                let state = &mut *this.account.state.borrow_mut();
                if state.suggested_timer != generation {
                    return;
                }
                let (Some((guild, users)), Some(me)) = (state.suggested.as_ref(), state.me()) else { return };
                state.member_check.suggested(*guild, users, me, &state.members, Instant::now());
            }
            this.wake_member_check();
        });
    }

    /// The suggestions closed, or list no one: a wait under way is dropped.
    pub(super) fn suggestions_closed(&self) {
        self.drop_suggested(&mut self.account.state.borrow_mut());
    }

    fn drop_suggested(&self, state: &mut super::state::State) {
        state.suggested = None;
        state.suggested_timer += 1;
    }

    /// Look at the check again when it next has something to do.
    pub(super) fn wake_member_check(self: &Rc<Self>) {
        let (generation, next) = {
            let mut state = self.account.state.borrow_mut();
            state.member_check_timer += 1;
            (state.member_check_timer, state.member_check.next())
        };
        let Some(next) = next else { return };
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(next.saturating_duration_since(Instant::now()), move || {
            if let Some(this) = this.upgrade()
                && this.account.epoch.get() == epoch
                && this.account.state.borrow().member_check_timer == generation
            {
                this.run_member_check();
            }
        });
    }

    /// Do what is due, and send the requests that are.
    fn run_member_check(self: &Rc<Self>) {
        {
            let state = &mut *self.account.state.borrow_mut();
            let connected = state.gateway.as_ref().is_some_and(|g| g.is_established());
            let epoch_ms = u64::try_from(super::state::now_ms()).unwrap_or(0);
            let requests =
                state.member_check.poll(Instant::now(), epoch_ms, &state.members, connected, glib::random_double);
            if let Some(gateway) = &state.gateway {
                for request in requests {
                    gateway.check_members(request.guild, &request.users, &request.nonce);
                }
            }
        }
        self.wake_member_check();
    }

    /// An answer to the check: whom it says left is forgotten.
    pub(super) fn members_checked(
        self: &Rc<Self>,
        guild: Snowflake,
        nonce: &str,
        found: &[Snowflake],
        not_found: &[Snowflake],
    ) {
        let (open, gone) = {
            let state = &mut *self.account.state.borrow_mut();
            let gone = state.member_check.answered(guild, nonce, found, not_found, &state.members, Instant::now());
            for &user in &gone {
                state.members.forget(guild, user);
            }
            (state.guild == Some(guild), !gone.is_empty())
        };
        if open && gone {
            self.refresh_rows();
            self.refresh_member_rows();
        }
        self.wake_member_check();
    }
}
