use std::rc::Rc;

use ergo_lib::chain::ergo_state_context::ErgoStateContext;
use ergo_lib::chain::transaction::unsigned::UnsignedTransaction;
use ergo_lib::chain::transaction::TxId;
use ergo_lib::ergo_chain_types::{Digest32, Header};
use ergo_lib::ergotree_interpreter::eval::env::Env;
use ergo_lib::ergotree_interpreter::sigma_protocol::private_input::PrivateInput;
use ergo_lib::ergotree_interpreter::sigma_protocol::prover::hint::HintsBag;
use ergo_lib::ergotree_interpreter::sigma_protocol::prover::{Prover, TestProver};
use ergo_lib::ergotree_ir::chain::address::Address;
use ergo_lib::ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergo_lib::ergotree_ir::chain::ergo_box::{ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters};
use ergo_lib::wallet::miner_fee::MINERS_FEE_ADDRESS;
use ergo_lib::wallet::secret_key::SecretKey;
use ergo_lib::wallet::signing::{make_context, TransactionContext};
use lazy_static::lazy_static;
use off_the_grid::grid::grid_order::{GridOrder, OrderState, MAX_FEE};

const HEADERS_JSON: &[u8] = include_bytes!("./headers.json");

lazy_static! {
    static ref HEADERS: Vec<Header> = serde_json::from_slice(HEADERS_JSON).unwrap();
}

fn generate_test_grid() -> (GridOrder, GridOrder, TestProver) {
    let secret_key = SecretKey::random_dlog();
    let prover = TestProver {
        secrets: vec![secret_key.clone().into()],
    };

    let group_element = if let PrivateInput::DlogProverInput(dpi) = PrivateInput::from(secret_key) {
        *dpi.public_image().h
    } else {
        panic!("Expected DlogProverInput")
    };

    let token = (Digest32::zero().into(), 1.try_into().unwrap()).into();

    let grid = GridOrder::new(
        group_element,
        1000000,
        1100000,
        token,
        OrderState::Buy,
        None,
    )
    .expect("Failed to create grid order");

    let filled = grid
        .clone()
        .into_filled()
        .expect("Failed to fill grid order");

    (grid, filled, prover)
}

fn create_fee_candidate(value: BoxValue) -> ErgoBoxCandidate {
    ErgoBoxCandidate {
        value,
        ergo_tree: MINERS_FEE_ADDRESS.script().unwrap(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height: 0,
    }
}

fn prove_input(
    input_box: ErgoBox,
    tx: UnsignedTransaction,
    prover: TestProver,
) -> Result<
    ergo_lib::ergotree_interpreter::sigma_protocol::prover::ProverResult,
    ergo_lib::ergotree_interpreter::sigma_protocol::prover::ProverError,
> {
    let tx_bytes = tx.bytes_to_sign().unwrap();

    let tx_context = TransactionContext::new(tx, vec![input_box.clone()], vec![]).unwrap();

    let state_context = ErgoStateContext::new(
        HEADERS[0].clone().into(),
        HEADERS.clone().try_into().unwrap(),
    );

    prover.prove(
        &input_box.ergo_tree,
        &Env::empty(),
        Rc::new(make_context(&state_context, &tx_context, 0).unwrap()),
        &tx_bytes,
        &HintsBag::empty(),
    )
}

#[test]
fn order_roundtrip() {
    let (grid, filled, _) = generate_test_grid();

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let filled_box_candidate = filled
        .clone()
        .into_box_candidate(0)
        .expect("Failed to create filled box candidate");

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![filled_box_candidate.clone(), fee_candidate.clone()],
    )
    .unwrap();

    prove_input(initial_box, tx, TestProver { secrets: vec![] }).expect("Failed to prove input");

    let filled_box = ErgoBox::from_box_candidate(&filled_box_candidate, TxId::zero(), 0).unwrap();

    let roundtripped = filled
        .into_filled()
        .expect("Failed to roundtrip filled order");

    let roundtripped_box_candidate = roundtripped
        .into_box_candidate(0)
        .expect("Failed to create roundtripped box candidate");

    let rountrip_tx = UnsignedTransaction::new_from_vec(
        vec![filled_box.clone().into()],
        vec![],
        vec![roundtripped_box_candidate, fee_candidate],
    )
    .expect("Failed to create roundtrip transaction");

    prove_input(filled_box, rountrip_tx, TestProver { secrets: vec![] })
        .expect("Failed to prove input");
}

#[test]
fn wrong_fee() {
    let (grid, filled, _) = generate_test_grid();

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let filled_box_candidate = filled
        .into_box_candidate(0)
        .expect("Failed to create filled box candidate");

    let fee_candidate = create_fee_candidate((MAX_FEE + 1).try_into().unwrap());

    // There are more nanoergs in the outputs but the prover does not check that.
    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![filled_box_candidate, fee_candidate],
    )
    .unwrap();

    // Force evaluation of the order filling path.
    let prover = TestProver { secrets: vec![] };

    let result = prove_input(initial_box, tx, prover);
    assert!(result.is_err());
}

#[test]
fn no_fee() {
    let (grid, filled, _) = generate_test_grid();

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let filled_box_candidate = filled
        .into_box_candidate(0)
        .expect("Failed to create filled box candidate");

    // There are more nanoergs in the outputs but the prover does not check that.
    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![filled_box_candidate],
    )
    .unwrap();

    // Force evaluation of the order filling path.
    let prover = TestProver { secrets: vec![] };

    let result = prove_input(initial_box, tx, prover);
    assert!(result.is_err());
}

#[test]
fn redeem_order() {
    let (grid, _, prover) = generate_test_grid();

    let address = if let PrivateInput::DlogProverInput(dpi) = &prover.secrets[0] {
        Address::P2Pk(dpi.public_image())
    } else {
        panic!("Expected DlogProverInput")
    };

    let value = grid.value;

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let redeem_box = ErgoBoxCandidate {
        value,
        ergo_tree: address.script().unwrap(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height: 0,
    };

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![redeem_box],
    )
    .unwrap();

    prove_input(initial_box, tx, prover).expect("Failed to prove input");
}

#[test]
fn redeem_wrong_secret() {
    let (grid, _, _) = generate_test_grid();
    let secret = SecretKey::random_dlog();

    let address = secret.get_address_from_public_image();

    let value = grid.value;

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let redeem_box = ErgoBoxCandidate {
        value,
        ergo_tree: address.script().unwrap(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height: 0,
    };

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![redeem_box],
    )
    .unwrap();

    let prover = TestProver {
        secrets: vec![secret.into()],
    };

    let result = prove_input(initial_box, tx, prover);
    assert!(result.is_err());
}

#[test]
fn filled_no_tokens() {
    let (grid, filled, _) = generate_test_grid();

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let mut filled_box_candidate = filled
        .into_box_candidate(0)
        .expect("Failed to create filled box candidate");

    filled_box_candidate.tokens = None;

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    // There are more nanoergs in the outputs but the prover does not check that.
    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![filled_box_candidate, fee_candidate],
    )
    .unwrap();

    // Force evaluation of the order filling path.
    let prover = TestProver { secrets: vec![] };

    let result = prove_input(initial_box, tx, prover);
    assert!(result.is_err());
}

#[test]
fn wrong_output_index() {
    let (grid, filled, _) = generate_test_grid();

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let filled_box_candidate = filled
        .into_box_candidate(0)
        .expect("Failed to create filled box candidate");

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![fee_candidate, filled_box_candidate],
    )
    .unwrap();

    // Force evaluation of the order filling path.
    let prover = TestProver { secrets: vec![] };

    let result = prove_input(initial_box, tx, prover);
    assert!(result.is_err());
}
