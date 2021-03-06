use super::state::{Block, Transaction};
use super::util;
use accumulator::group::UnknownOrderGroup;
use accumulator::{AccError, Accumulator};
use multiqueue::{BroadcastReceiver, BroadcastSender};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

/// A stateless miner in our system.
pub struct Miner<G: UnknownOrderGroup, T: Clone + Hash + Debug> {
    acc: Accumulator<G, T>,
    block_height: u64,
    pending_transactions: Vec<Transaction<G, T>>,
}

impl<G: UnknownOrderGroup, T: 'static + Clone + Eq + Hash + Debug + PartialEq + Send> Miner<G, T> {
    /// Runs a miner's simulation loop.
    // Assumes all miners are online from genesis. We may want to implement syncing later.
    pub fn start(
        is_leader: bool,
        acc: Accumulator<G, T>,
        block_interval_ms: u64,
        block_sender: &BroadcastSender<Block<G, T>>,
        block_receiver: BroadcastReceiver<Block<G, T>>,
        tx_receiver: BroadcastReceiver<Transaction<G, T>>,
    ) {
        let miner_ref = Arc::new(Mutex::new(Self {
            acc,
            block_height: 0,
            pending_transactions: Vec::new(),
        }));

        // Transaction processor thread.
        let miner = miner_ref.clone();
        let transaction_thread = thread::spawn(move || loop {
            match tx_receiver.try_recv() {
                Ok(tx) => miner.lock().unwrap().add_transaction(tx),
                Err(_) => (),
            }
            sleep(Duration::from_millis(10));
        });

        // Block validation thread.
        let miner = miner_ref.clone();
        let validate_thread = thread::spawn(move || loop {
            match block_receiver.try_recv() {
                Ok(block) => miner.lock().unwrap().validate_block(block),
                Err(_) => (),
            }
            sleep(Duration::from_millis(10));
        });

        // Block creation on an interval.
        if is_leader {
            loop {
                sleep(Duration::from_millis(block_interval_ms));
                let new_block = miner_ref.lock().unwrap().forge_block();
                if let Ok(block) = new_block {
                    block_sender.try_send(block).unwrap();
                } else {
                    println!("Fail on forging block");
                }
            }
        }

        transaction_thread.join().unwrap();
        validate_thread.join().unwrap();
    }

    fn add_transaction(&mut self, transaction: Transaction<G, T>) {
        // This `contains` check could incur overhead; ideally we'd use a set but Rust `HashSet` is
        // kind of a pain to use here.
        if !self.pending_transactions.contains(&transaction) {
            self.pending_transactions.push(transaction);
        }
    }

    fn forge_block(&self) -> Result<Block<G, T>, AccError> {
        let (elems_added, elems_deleted) =
            util::elems_from_transactions(&self.pending_transactions);
        println!(
            "Forging block {} with {} elems added and {} elems deleted.",
            self.block_height + 1,
            elems_added.len(),
            elems_deleted.len()
        );
        let (witness_deleted, proof_deleted) =
            self.acc.clone().delete_with_proof(&elems_deleted)?;
        let (acc_new, proof_added) = witness_deleted.clone().add_with_proof(&elems_added);
        let new_block = Block {
            height: self.block_height + 1,
            transactions: self.pending_transactions.clone(),
            acc_new,
            proof_added,
            proof_deleted,
        };
//        println!(
//            "No.{} forged block: {:#?}",
//            self.block_height + 1,
//            new_block
//        );
        Ok(new_block)
    }

    fn validate_block(&mut self, block: Block<G, T>) {
        // Preserves idempotency if multiple miners are leaders.
        if block.height != self.block_height + 1 {
            return;
        }

        let (elems_added, elem_witnesses_deleted) =
            util::elems_from_transactions(&block.transactions);
        let elems_deleted: Vec<T> = elem_witnesses_deleted
            .iter()
            .map(|(u, _wit)| u.clone())
            .collect();
        assert!(self
            .acc
            .verify_membership_batch(&elems_deleted, &block.proof_deleted));
        assert!(block
            .acc_new
            .verify_membership_batch(&elems_added, &block.proof_added));
        assert_eq!(block.proof_deleted.witness, block.proof_added.witness);
        self.acc = block.acc_new.clone();
        self.block_height = block.height;
        self.pending_transactions.clear();
    }
}
