use async_channel::Receiver;

pub trait ReceiverCleanup<T> {
    fn close_and_drain(&self) -> usize;
}

impl<T> ReceiverCleanup<T> for Receiver<T> {
    fn close_and_drain(&self) -> usize {
        self.close();

        let mut drained = 0;
        while self.try_recv().is_ok() {
            drained += 1;
        }

        drained
    }
}
