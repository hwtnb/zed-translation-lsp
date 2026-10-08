use super::worker::Job;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Notify;

struct Queue {
    jobs: VecDeque<Job>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Notify,
    capacity: usize,
}

pub(super) struct JobSender(Arc<Shared>);
pub(super) struct JobReceiver(Arc<Shared>);

#[derive(Debug)]
pub(super) enum SendError {
    Full,
    Closed,
}

pub(super) fn channel(capacity: usize) -> (JobSender, JobReceiver) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            jobs: VecDeque::new(),
            closed: false,
        }),
        ready: Notify::new(),
        capacity,
    });
    (JobSender(shared.clone()), JobReceiver(shared))
}

fn live(job: &Job) -> bool {
    job.deadline.has_changed().is_ok()
        && job
            .deadline
            .borrow()
            .is_some_and(|deadline| deadline > Instant::now())
}

impl JobSender {
    pub fn try_send(&self, job: Job) -> Result<(), SendError> {
        let mut queue = self.0.queue.lock().unwrap();
        if queue.closed {
            return Err(SendError::Closed);
        }
        // The worker may be occupied. Reclaim retired jobs before admission,
        // retaining the FIFO order of every request that still has an owner.
        queue.jobs.retain(live);
        if queue.jobs.len() == self.0.capacity {
            return Err(SendError::Full);
        }
        queue.jobs.push_back(job);
        drop(queue);
        self.0.ready.notify_one();
        Ok(())
    }
}

impl JobReceiver {
    pub async fn recv(&mut self) -> Option<Job> {
        loop {
            let ready = self.0.ready.notified();
            {
                let mut queue = self.0.queue.lock().unwrap();
                while let Some(job) = queue.jobs.pop_front() {
                    if live(&job) {
                        return Some(job);
                    }
                }
                if queue.closed {
                    return None;
                }
            }
            ready.await;
        }
    }
}

impl Drop for JobSender {
    fn drop(&mut self) {
        self.0.queue.lock().unwrap().closed = true;
        self.0.ready.notify_one();
    }
}

impl Drop for JobReceiver {
    fn drop(&mut self) {
        let mut queue = self.0.queue.lock().unwrap();
        queue.closed = true;
        queue.jobs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lsp::server::worker::Task,
        providers::{Context, Intent},
    };
    use std::time::Duration;
    use tokio::sync::watch;

    fn job(ticket: u64) -> (watch::Sender<Option<Instant>>, Job) {
        let (stop, deadline) = watch::channel(Some(Instant::now() + Duration::from_secs(10)));
        (
            stop,
            Job {
                ticket,
                deadline,
                task: Task::Translation {
                    intent: Intent::Explicit,
                    text: String::new(),
                    target_language: "ja".into(),
                    context: Context::Mock,
                },
            },
        )
    }

    #[tokio::test]
    async fn admission_reclaims_retired_jobs_and_preserves_live_fifo_order() {
        for expired in [false, true] {
            let (sender, mut receiver) = channel(2);
            let (first_stop, first) = job(1);
            let (_second_stop, second) = job(2);
            sender.try_send(first).unwrap();
            sender.try_send(second).unwrap();
            let (_third_stop, third) = job(3);
            assert!(matches!(sender.try_send(third), Err(SendError::Full)));
            first_stop.send_replace(if expired { Some(Instant::now()) } else { None });
            let (_fourth_stop, fourth) = job(4);
            sender.try_send(fourth).unwrap();
            assert_eq!(receiver.recv().await.unwrap().ticket, 2);
            assert_eq!(receiver.recv().await.unwrap().ticket, 4);
            drop(sender);
            assert!(receiver.recv().await.is_none());
        }
    }

    #[tokio::test]
    async fn closing_either_endpoint_releases_its_peer() {
        let (sender, mut receiver) = channel(1);
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(receiver.recv(), async {
                tokio::task::yield_now().await;
                drop(sender);
            })
        })
        .await
        .unwrap();
        assert!(result.is_none());
        let (sender, receiver) = channel(1);
        drop(receiver);
        let (_stop, job) = job(1);
        assert!(matches!(sender.try_send(job), Err(SendError::Closed)));
    }
}
