use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::i18n::{Language, Message};

/// How long a call waits for a person to answer before it gives up. It has to
/// stay below `crate::protocol::CALL_TIMEOUT`, the point where the bridge stops
/// believing in a reply: waiting longer would turn an unfinished approval into
/// an unknown-outcome write.
pub const WAIT: Duration = Duration::from_secs(15);

/// An answered request is kept this long so the retry that follows a waiter
/// timeout picks the answer up instead of asking the person twice.
const ANSWER_TTL: Duration = Duration::from_secs(90);

/// How long an unanswered prompt stays on screen. It outlives the call that
/// raised it, so a person answering late is still honoured by the retry, but it
/// does not sit there indefinitely holding the keys the prompt shadows.
const LINGER: Duration = Duration::from_secs(60);

/// One answered request, or one waiting request, whichever is older.
const MAX_SLOTS: usize = 8;

/// A call a person has to allow or refuse, described only by fields the AI was
/// already authorized to see plus the arguments it asked to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub grant: String,
    pub tool: String,
    pub repository: String,
    pub is_write: bool,
    /// The arguments as one short line. Truncated, because a body can be 32 KiB.
    pub preview: String,
    pub asked_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Denied,
}

impl Request {
    /// The call in one line, for whoever is holding the terminal. Only fields
    /// the grant already allowed the AI to see, plus the arguments it sent.
    pub fn describe(&self, lang: Language) -> String {
        let kind = lang.text(if self.is_write {
            Message::ApprovalWriteKind
        } else {
            Message::ApprovalReadKind
        });
        lang.format(
            Message::ApprovalRequestLine,
            &[
                ("grant", &self.grant as &dyn std::fmt::Display),
                ("tool", &self.tool),
                ("kind", &kind),
                ("repository", &self.repository),
            ],
        )
    }

    pub fn arguments(&self, lang: Language) -> String {
        lang.format(Message::ApprovalArguments, &[("preview", &self.preview)])
    }
}

/// How long the person gets to answer, phrased for the terminal asking.
pub fn ask_hint(lang: Language) -> String {
    lang.format(Message::ApprovalAsk, &[("seconds", &WAIT.as_secs())])
}

/// Handed to the waiting call. Sharing the answer cell is what lets a retry
/// after a timeout reuse the decision a person already made.
#[derive(Clone)]
pub struct Ticket {
    pub id: u64,
    answer: Arc<Mutex<Option<Decision>>>,
}

impl Ticket {
    pub fn answer(&self) -> Option<Decision> {
        *self.answer.lock().ok()?
    }
}

#[derive(Clone)]
struct Slot {
    request: Request,
    key: String,
    answer: Arc<Mutex<Option<Decision>>>,
    answered_at: Option<Instant>,
}

/// The approvals a broker process is holding. Deliberately in memory only: the
/// process that owns the unlocked vault is the process that asks the person, so
/// answering needs no local channel the AI could write to.
#[derive(Default)]
pub struct ApprovalQueue {
    next_id: AtomicU64,
    slots: Mutex<Vec<Slot>>,
}

impl ApprovalQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers a call for approval, or reuses the request already on screen
    /// for the same call. `key` binds capability, operation and arguments.
    pub fn ask(&self, key: &str, request: Request) -> Ticket {
        let mut slots = self.slots.lock().expect("approval queue is not reentrant");
        prune(&mut slots);
        if let Some(index) = slots.iter().position(|slot| slot.key == key) {
            let mut slot = slots[index].clone();
            // A decision already made is spent by this one retry.
            if slot.answered_at.is_some() {
                slots.remove(index);
            } else {
                // A retry of a call that timed out is a fresh ask about the same
                // arguments, so the prompt gets its whole window again.
                slot.request.asked_at = Instant::now();
                slots[index] = slot.clone();
            }
            return Ticket {
                id: slot.request.id,
                answer: slot.answer,
            };
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = request;
        request.id = id;
        let slot = Slot {
            request,
            key: key.to_owned(),
            answer: Arc::new(Mutex::new(None)),
            answered_at: None,
        };
        let ticket = Ticket {
            id,
            answer: slot.answer.clone(),
        };
        slots.push(slot);
        if slots.len() > MAX_SLOTS {
            // The oldest unanswered request loses its place; its waiter is told.
            if let Some(index) = slots.iter().position(|slot| slot.answered_at.is_none()) {
                let slot = slots.remove(index);
                if let Ok(mut answer) = slot.answer.lock() {
                    *answer = Some(Decision::Denied);
                }
            }
        }
        ticket
    }

    /// Requests a person has not answered yet, oldest first.
    pub fn waiting(&self) -> Vec<Request> {
        let mut slots = self.slots.lock().expect("approval queue is not reentrant");
        prune(&mut slots);
        let mut waiting: Vec<Request> = slots
            .iter()
            .filter(|slot| slot.answered_at.is_none())
            .map(|slot| slot.request.clone())
            .collect();
        waiting.sort_by_key(|request| request.id);
        waiting
    }

    /// Records a person's answer. False means the request is no longer there.
    /// The answered slot stays behind briefly on purpose: a decision belongs to
    /// the call it was made for, and the attempt that follows a timeout is that
    /// same call coming back. So one decision answers exactly one further attempt,
    /// whether it was a yes or a no, and the attempt after that is a fresh question.
    pub fn decide(&self, id: u64, decision: Decision) -> bool {
        let mut slots = self.slots.lock().expect("approval queue is not reentrant");
        prune(&mut slots);
        let Some(index) = slots
            .iter()
            .position(|slot| slot.request.id == id && slot.answered_at.is_none())
        else {
            return false;
        };
        let Ok(mut answer) = slots[index].answer.lock() else {
            return false;
        };
        if answer.is_some() {
            return false;
        }
        *answer = Some(decision);
        drop(answer);
        slots[index].answered_at = Some(Instant::now());
        true
    }

    /// Refuses everything still waiting, because the broker is going away.
    pub fn deny_all(&self) -> usize {
        let mut slots = self.slots.lock().expect("approval queue is not reentrant");
        let denied = slots.len();
        for slot in slots.iter() {
            if let Ok(mut answer) = slot.answer.lock() {
                answer.get_or_insert(Decision::Denied);
            }
        }
        slots.clear();
        denied
    }

    pub fn is_empty(&self) -> bool {
        self.slots
            .lock()
            .is_ok_and(|slots| slots.iter().all(|slot| slot.answered_at.is_some()))
    }
}

fn prune(slots: &mut Vec<Slot>) {
    slots.retain(|slot| match slot.answered_at {
        Some(at) => at.elapsed() < ANSWER_TTL,
        None => slot.request.asked_at.elapsed() < LINGER,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ApprovalPolicy;

    fn request(tool: &str) -> Request {
        Request {
            id: 0,
            grant: "test-agent".to_owned(),
            tool: tool.to_owned(),
            repository: "example/project".to_owned(),
            is_write: tool.contains("create"),
            preview: format!("{tool} example/project"),
            asked_at: Instant::now(),
        }
    }

    #[test]
    fn a_late_answer_is_honoured_once_and_the_next_attempt_is_asked_again() {
        let queue = ApprovalQueue::new();
        let waiter = queue.ask("k", request("github_list_issues"));
        assert_eq!(queue.waiting().len(), 1);
        assert!(queue.decide(waiter.id, Decision::Approved));
        assert!(
            queue.waiting().is_empty(),
            "an answered prompt is a question no longer asked"
        );
        assert!(queue.is_empty(), "an answered slot is not a waiting call");
        assert_eq!(waiter.answer(), Some(Decision::Approved));

        // A call that gave up waiting is retried by the AI as a fresh registration;
        // it inherits the decision that was made for it instead of asking twice.
        let retry = queue.ask("k", request("github_list_issues"));
        assert_eq!(retry.id, waiter.id);
        assert_eq!(retry.answer(), Some(Decision::Approved));
        // That answer is spent by the one retry, so a third attempt is a new question.
        let again = queue.ask("k", request("github_list_issues"));
        assert_ne!(again.id, waiter.id);
        assert_eq!(again.answer(), None);
        assert_eq!(queue.waiting().len(), 1);
    }

    #[test]
    fn an_answer_only_lands_on_the_request_it_was_given_for() {
        let queue = ApprovalQueue::new();
        let kept = queue.ask("a", request("github_list_issues"));
        let other = queue.ask("b", request("github_create_issue"));
        assert_ne!(kept.id, other.id);
        assert!(!queue.decide(kept.id + 99, Decision::Approved));
        assert!(queue.decide(other.id, Decision::Approved));
        assert_eq!(other.answer(), Some(Decision::Approved));
        assert_eq!(kept.answer(), None);
        // Answering twice is not a second decision.
        assert!(!queue.decide(other.id, Decision::Denied));
        assert_eq!(other.answer(), Some(Decision::Approved));
    }

    #[test]
    fn the_oldest_unanswered_prompt_loses_its_place_when_the_screen_is_full() {
        let queue = ApprovalQueue::new();
        let mut tickets = Vec::new();
        for index in 0..MAX_SLOTS {
            tickets.push(queue.ask(&format!("k{index}"), request("github_list_issues")));
        }
        assert_eq!(queue.waiting().len(), MAX_SLOTS);
        let overflow = queue.ask("overflow", request("github_list_issues"));
        assert_eq!(queue.waiting().len(), MAX_SLOTS);
        assert_eq!(
            tickets[0].answer(),
            Some(Decision::Denied),
            "a prompt that left the screen must be refused, not left hanging"
        );
        assert_eq!(overflow.answer(), None);
        assert_ne!(tickets[0].id, overflow.id);
    }

    #[test]
    fn locking_refuses_everything_still_waiting() {
        let queue = ApprovalQueue::new();
        let read = queue.ask("a", request("github_list_issues"));
        let write = queue.ask("b", request("github_create_issue"));
        assert_eq!(queue.deny_all(), 2);
        assert_eq!(read.answer(), Some(Decision::Denied));
        assert_eq!(write.answer(), Some(Decision::Denied));
        assert!(queue.waiting().is_empty());
        assert_eq!(queue.deny_all(), 0, "nothing is left to answer");
    }

    #[test]
    fn the_policy_says_which_calls_a_person_sees() {
        assert!(!ApprovalPolicy::Off.requires(true));
        assert!(!ApprovalPolicy::Off.requires(false));
        assert!(ApprovalPolicy::Write.requires(true));
        assert!(!ApprovalPolicy::Write.requires(false));
        assert!(ApprovalPolicy::All.requires(true));
        assert!(ApprovalPolicy::All.requires(false));
        assert_eq!(ApprovalPolicy::default(), ApprovalPolicy::Off);
        assert_eq!(ApprovalPolicy::Write.name(), "write");
    }

    #[test]
    fn a_prompt_describes_the_call_without_leaking_more_than_the_grant_allows() {
        let queue = ApprovalQueue::new();
        let ticket = queue.ask(
            "k",
            Request {
                preview: "{\"repository\":\"example/project\",\"title\":\"Ship it\"}".to_owned(),
                ..request("github_create_issue")
            },
        );
        for lang in [Language::ZhCn, Language::En] {
            let request = queue.waiting()[0].clone();
            let line = request.describe(lang);
            assert!(line.contains("test-agent"));
            assert!(line.contains(&request.tool));
            assert!(line.contains("example/project"));
            let arguments = request.arguments(lang);
            assert!(arguments.contains("Ship it"));
            assert!(ask_hint(lang).contains(&WAIT.as_secs().to_string()));
        }
        assert_eq!(ticket.answer(), None);
    }
}
