use chrono::Utc;
pub use commucat_proto::call::{CallAnswer, CallEnd, CallOffer, CallStats};
use std::collections::HashMap;

#[derive(Default)]
pub struct CallManager {
    active_calls: HashMap<String, ActiveCall>,
}

#[derive(Debug, Clone)]
pub struct ActiveCall {
    pub offer: CallOffer,
    pub answer: Option<CallAnswer>,
    pub stats: Vec<CallStats>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl CallManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_offer(&mut self, offer: CallOffer) {
        self.active_calls
            .entry(offer.call_id.clone())
            .and_modify(|call| {
                call.offer = offer.clone();
                call.started_at = None;
                call.ended_at = None;
            })
            .or_insert_with(|| ActiveCall {
                offer,
                answer: None,
                stats: Vec::new(),
                started_at: None,
                ended_at: None,
            });
    }

    pub fn accept_answer(&mut self, answer: CallAnswer) -> bool {
        if let Some(call) = self.active_calls.get_mut(&answer.call_id) {
            call.answer = Some(answer.clone());
            if answer.accept {
                call.started_at = Some(Utc::now().timestamp());
            } else {
                call.ended_at = Some(Utc::now().timestamp());
            }
            true
        } else {
            false
        }
    }

    pub fn end_call(&mut self, call_id: &str) -> bool {
        if let Some(call) = self.active_calls.get_mut(call_id) {
            call.ended_at = Some(Utc::now().timestamp());
            true
        } else {
            false
        }
    }

    pub fn push_stats(&mut self, stats: CallStats) {
        if let Some(call) = self.active_calls.get_mut(&stats.call_id) {
            call.stats.push(stats);
            if call.stats.len() > 256 {
                call.stats.drain(..call.stats.len() - 256);
            }
        }
    }

    pub fn get_active_calls(&self) -> Vec<String> {
        self.active_calls
            .iter()
            .filter(|(_, call)| call.started_at.is_some() && call.ended_at.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn get_call(&self, call_id: &str) -> Option<&ActiveCall> {
        self.active_calls.get(call_id)
    }
}
