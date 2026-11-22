//! Keep in-flight mutations alive across reconnects and replay their result.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::Mutex,
};

use tokio::sync::watch;
use tonic::Status;

const CAPACITY: usize = 64;
type ResultSlot<R> = watch::Receiver<Option<Result<R, Status>>>;

pub(super) struct Cache<I, R> {
    entries: Mutex<Entries<I, R>>,
}

struct Entries<I, R> {
    map: HashMap<String, (I, ResultSlot<R>)>,
    order: VecDeque<String>,
}

impl<I: Eq, R: Clone + Send + Sync + 'static> Cache<I, R> {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(Entries {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub(super) async fn run<F>(
        &self,
        id: &str,
        input: I,
        perform: impl FnOnce() -> F,
    ) -> Result<R, Status>
    where
        F: Future<Output = Result<R, Status>> + Send + 'static,
    {
        let id = request_id(id)?;
        let mut result = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some((prior, result)) = entries.map.get(&id) {
                if prior != &input {
                    return Err(Status::already_exists(
                        "request ID was used for a different mutation",
                    ));
                }
                result.clone()
            } else {
                entries.reserve()?;
                let (output, result) = watch::channel(None);
                entries.order.push_back(id.clone());
                entries.map.insert(id, (input, result.clone()));
                let operation = perform();
                tokio::spawn(async move {
                    output.send_replace(Some(operation.await));
                });
                result
            }
        };
        let value = result
            .wait_for(Option::is_some)
            .await
            .map_err(|_| Status::internal("daemon mutation ended without a result"))?;
        value
            .clone()
            .ok_or_else(|| Status::internal("daemon mutation result was lost"))?
    }
}

impl<I, R> Entries<I, R> {
    fn reserve(&mut self) -> Result<(), Status> {
        if self.map.len() < CAPACITY {
            return Ok(());
        }
        let completed = self.order.iter().position(|id| {
            self.map.get(id).is_some_and(|(_, result)| {
                result.borrow().is_some() || result.has_changed().is_err()
            })
        });
        let oldest = completed
            .and_then(|index| self.order.remove(index))
            .ok_or_else(|| Status::resource_exhausted("the daemon mutation queue is full"))?;
        self.map.remove(&oldest);
        Ok(())
    }
}

pub(super) fn request_id(value: &str) -> Result<String, Status> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
    {
        return Err(Status::invalid_argument("request ID must be a UUID"));
    }
    Ok(value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn disconnected_caller_can_rejoin_the_same_existing_operation() {
        let cache = std::sync::Arc::new(Cache::new());
        let id = "10000000-0000-4000-8000-000000000001";
        let (entered, await_entered) = oneshot::channel();
        let (complete, await_completion) = oneshot::channel();
        let other = cache.clone();
        let caller = tokio::spawn(async move {
            other
                .run(id, (), || async move {
                    entered.send(()).unwrap();
                    Ok(await_completion.await.unwrap())
                })
                .await
        });
        await_entered.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());

        complete.send(5).unwrap();
        let result = cache.run(id, (), || async { Ok(99) }).await.unwrap();
        assert_eq!(result, 5);
    }
}
