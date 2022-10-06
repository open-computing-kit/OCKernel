//! simple but hopefully scalable timer framework

use crate::arch::Registers;
use alloc::{
    collections::VecDeque,
    vec::Vec,
};
use core::sync::atomic;
use log::{warn, trace};

pub type TimerCallback = fn(&mut Registers);

struct Timer {
    expires_at: u64,
    callback: TimerCallback,
}

pub struct TimerState {
    jiffies: u64,
    hz: u64,
    timers: VecDeque<Timer>,
    lock: atomic::AtomicBool,
}

#[derive(Debug)]
pub struct TimerAddError;

impl TimerState {
    fn new(hz: u64) -> Self {
        Self {
            jiffies: 0,
            hz,
            timers: VecDeque::new(),
            lock: atomic::AtomicBool::new(false),
        }
    }

    fn take_lock(&self) {
        if self.lock.swap(true, atomic::Ordering::Acquire) {
            warn!("timer state is locked, spinning");
            while self.lock.swap(true, atomic::Ordering::Acquire) {}
        }
    }

    fn release_lock(&self) {
        self.lock.store(false, atomic::Ordering::Release);
    }

    /// ticks the timer, incrementing its jiffies counter and calling any callbacks that are due
    pub fn tick(&mut self, registers: &mut Registers) {
        self.tick_no_callbacks();

        // run callbacks for all expired timers

        self.take_lock();

        while let Some(timer) = self.timers.front() {
            if self.jiffies >= timer.expires_at {
                let callback = self.timers.pop_front().unwrap().callback;

                trace!("timer timed out at {}, {} more timers", self.jiffies, self.timers.len());

                self.release_lock();

                (callback)(registers);

                self.take_lock();
            } else {
                // break out of the loop since we keep the timer queue sorted
                break;
            }
        }

        self.release_lock();
    }

    /// ticks the timer without running any callbacks (may be useful if things are locked? idk)
    pub fn tick_no_callbacks(&mut self) {
        self.jiffies += 1;
    }

    /// returns the current jiffies counter of the timer
    pub fn jiffies(&self) -> u64 {
        self.jiffies
    }

    /// returns the timer's hz value (how many ticks per second)
    pub fn hz(&self) -> u64 {
        self.hz
    }

    /// adds a timer that expires at the given time
    pub fn add_timer_at(&mut self, expires_at: u64, callback: TimerCallback) -> Result<(), TimerAddError> {
        if expires_at <= self.jiffies {
            Err(TimerAddError)
        } else {
            let timer = Timer { expires_at, callback };

            self.take_lock();

            if self.timers.try_reserve(1).is_err() {
                self.release_lock();
                Err(TimerAddError)?;
            }

            match self.timers.iter().position(|t| t.expires_at >= expires_at) { // keep the timer queue sorted
                Some(index) => self.timers.insert(index, timer),
                None => self.timers.push_back(timer),
            }

            self.release_lock();

            Ok(())
        }
    }

    /// adds a timer that expires in the given number of ticks from when it was added
    pub fn add_timer_in(&mut self, expires_in: u64, callback: TimerCallback) -> Result<u64, TimerAddError> {
        let expires_at = self.jiffies + expires_in;
        self.add_timer_at(expires_at, callback)?;
        Ok(expires_at)
    }

    /// removes a timer, given its expiration time
    pub fn remove_timer(&mut self, expires_at: u64) {
        self.take_lock();

        if let Some(index) = self.timers.iter().position(|t| t.expires_at == expires_at) {
            self.timers.remove(index);
        }

        self.release_lock();
    }
}

/// all of our timers
static mut TIMER_STATES: Vec<TimerState> = Vec::new();

/// used to lock TIMER_STATES while we're adding a timer
static ADD_TIMER_LOCK: atomic::AtomicBool = atomic::AtomicBool::new(false);

#[derive(Debug)]
pub struct TimerRegisterError;

/// registers a new timer with the given tick rate
pub fn register_timer(hz: u64) -> Result<usize, TimerRegisterError> {
    // acquire the lock
    if ADD_TIMER_LOCK.swap(true, atomic::Ordering::Acquire) {
        warn!("timer states are locked, spinning");
        while ADD_TIMER_LOCK.swap(true, atomic::Ordering::Acquire) {}
    }

    let result = unsafe {
        if TIMER_STATES.try_reserve(1).is_err() {
            Err(TimerRegisterError)
        } else {
            let next_timer = TIMER_STATES.len();
            TIMER_STATES.push(TimerState::new(hz));

            Ok(next_timer)
        }
    };

    // release the lock
    ADD_TIMER_LOCK.store(false, atomic::Ordering::Release);

    result
}

/// gets a registered timer
pub fn get_timer(index: usize) -> Option<&'static mut TimerState> {
    // no need to lock here since timer states handle their own locking
    unsafe {
        TIMER_STATES.get_mut(index)
    }
}