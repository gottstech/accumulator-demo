use super::state::Transaction;
use super::state::Utxo;
use crate::simulation::bridge::{UserUpdate, WitnessRequest, WitnessResponse};
use accumulator::group::UnknownOrderGroup;
use multiqueue::{BroadcastReceiver, BroadcastSender};
use rand::Rng;
use std::collections::HashSet;
use std::thread::sleep;
use std::time::Duration;
use uuid::Uuid;

/// A end-user or light-client in our system.
pub struct User {
    id: usize, // For bridges to know who to send witness responses to.
    utxo_set: HashSet<Utxo>,
}

impl User {
    /// Runs a user's simulation loop.
    // Right now users are limited to one transaction per block (i.e. they can issue one transaction
    // based on their UTXO set as of some block), since users have to wait for their state to be
    // updated before issuing a subsequent transaction. TODO: Allow for more tx per user per block.
    pub fn start<G: 'static + UnknownOrderGroup>(
        id: usize,
        bridge_id: usize,
        init_utxo: Utxo,
        witness_request_sender: &BroadcastSender<WitnessRequest>,
        witness_response_receiver: &BroadcastReceiver<WitnessResponse<G, Utxo>>,
        user_update_receiver: &BroadcastReceiver<UserUpdate>,
        tx_sender: &BroadcastSender<Transaction<G, Utxo>>,
    ) {
        let mut utxo_set = HashSet::new();
        utxo_set.insert(init_utxo);
        let mut user = Self { id, utxo_set };

        loop {
            sleep(Duration::from_millis(10));

            // Get a UTXO to spend.
            let mut utxos_to_spend = Vec::new();
            utxos_to_spend.push(user.get_input_for_transaction());

            // Request a witness for the UTXO we are spending.
            let response = {
                let witness_request_id = Uuid::new_v4();
                loop {
                    witness_request_sender
                        .try_send(WitnessRequest {
                            user_id: user.id,
                            request_id: witness_request_id,
                            utxos: utxos_to_spend.clone(),
                        })
                        .unwrap();

                    let response = loop {
                        match witness_response_receiver.try_recv() {
                            Ok(response) => break response,
                            Err(_) => (),
                        }
                        sleep(Duration::from_millis(10));
                    };
                    if response.request_id == witness_request_id {
                        break response;
                    }
                    // Drain any other responses so we don't loop forever.
                    loop {
                        if witness_response_receiver.try_recv().is_err() {
                            break;
                        }
                    }
                }
            };

            let num = rand::thread_rng().gen_range(1, 3);
            let mut new_utxos = vec![];
            for _ in 0..num {
                new_utxos.push(Utxo {
                    id: Uuid::new_v4(),
                    user_id: user.id,
                });
            }

            let new_trans = Transaction {
                utxos_created: new_utxos,
                utxos_spent_with_witnesses: response.utxos_with_witnesses,
            };

            // Issue a transaction to miners.
            tx_sender.try_send(new_trans).unwrap();
            println!("User {} for bridge {} issued transaction.", id, bridge_id,);

            // Keep processing UTXO updates from the bridge until one of them is non-empty (i.e. the
            // one we care about, pertaining to the UTXO we spent).
            loop {
                match user_update_receiver.try_recv() {
                    Ok(update) => if !update.is_empty() {
                        user.update(update);
                        break;
                    }
                    Err(_) => (),
                }
                sleep(Duration::from_millis(10));
            }
        }
    }

    // TODO: Maybe support more inputs than one.
    // Expects executable to call `update` to remove this UTXO when it is confirmed.
    fn get_input_for_transaction(&self) -> Utxo {
        self.utxo_set.iter().next().unwrap().clone()
    }

    fn update(&mut self, update: UserUpdate) {
        for utxo in update.utxos_deleted {
            self.utxo_set.remove(&utxo);
        }
        for utxo in update.utxos_added {
            self.utxo_set.insert(utxo.clone());
        }
    }
}
