use std::rc::Rc;

use ergo_lib::{
    chain::{
        ergo_state_context::ErgoStateContext,
        transaction::{unsigned::UnsignedTransaction, TxId},
    },
    ergo_chain_types::{Digest, Digest32, Header},
    ergotree_interpreter::{
        eval::env::Env,
        sigma_protocol::{
            private_input::PrivateInput,
            prover::{hint::HintsBag, Prover, TestProver},
        },
    },
    ergotree_ir::chain::{
        address::Address,
        ergo_box::{box_value::BoxValue, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters},
        token::{Token, TokenId},
    },
    wallet::{
        miner_fee::MINERS_FEE_ADDRESS,
        secret_key::SecretKey,
        signing::{make_context, TransactionContext},
    },
};
use lazy_static::lazy_static;

use off_the_grid::grid::{
    grid_order::OrderState,
    multigrid_order::{GridOrderEntries, GridOrderEntry, MultiGridOrder, MAX_FEE},
};

const HEADERS_JSON: &[u8] = include_bytes!("./headers.json");

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

lazy_static! {
    static ref HEADERS: Vec<Header> = serde_json::from_slice(HEADERS_JSON).unwrap();
}

fn generate_test_grid() -> (MultiGridOrder, TestProver) {
    let secret_key = SecretKey::random_dlog();
    let prover = TestProver {
        secrets: vec![secret_key.clone().into()],
    };

    let group_element = if let PrivateInput::DlogProverInput(dpi) = PrivateInput::from(secret_key) {
        *dpi.public_image().h
    } else {
        panic!("Expected DlogProverInput")
    };

    let token_id: TokenId = Digest32::zero().into();

    let start = 10u64;
    let end = 100u64;
    let step = 10usize;

    let entries = (start..=end)
        .step_by(step)
        .map(|i| GridOrderEntry {
            state: OrderState::Buy,
            token_amount: 1u64.try_into().unwrap(),
            bid_value: i,
            ask_value: i + step as u64,
        })
        .collect();

    let entries = GridOrderEntries::new(entries);

    let grid = MultiGridOrder::new(group_element, token_id, entries, None).unwrap();

    (grid, prover)
}

#[test]
fn redeem_order() {
    let (grid, prover) = generate_test_grid();

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

    prove_input(initial_box.clone(), tx.clone(), prover).expect("Failed to prove input");

    prove_input(initial_box, tx, TestProver { secrets: vec![] })
        .expect_err("Should fail to prove input");
}

#[test]
fn swap_single_roundtrip() {
    let (grid, _) = generate_test_grid();

    let entries = grid
        .entries
        .clone()
        .into_fill_bid()
        .expect("Failed to fill bid");

    let swapped = grid
        .clone()
        .with_entries(entries)
        .expect("Failed to perform swaps");

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let swapped_candidate = swapped
        .clone()
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![swapped_candidate.clone(), fee_candidate.clone()],
    )
    .unwrap();

    println!("tx: {}", serde_json::to_string_pretty(&tx).unwrap());

    prove_input(initial_box, tx, TestProver { secrets: vec![] }).expect("Failed to prove input");

    let swapped_box = ErgoBox::from_box_candidate(&swapped_candidate, TxId::zero(), 0).unwrap();

    let entries = swapped
        .entries
        .clone()
        .into_fill_ask()
        .expect("Failed to fill ask");

    let roundtripped = swapped.with_entries(entries).expect("Failed to swap");

    let roundtripped_candidate = roundtripped
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let roundtrip_tx = UnsignedTransaction::new_from_vec(
        vec![swapped_box.clone().into()],
        vec![],
        vec![roundtripped_candidate, fee_candidate],
    )
    .unwrap();

    MultiGridOrder::try_from(&swapped_box).expect("Failed to parse swapped box");

    prove_input(swapped_box, roundtrip_tx, TestProver { secrets: vec![] })
        .expect("Failed to prove input");
}

#[test]
fn swap_multiple_roundtrip() {
    let (grid, _) = generate_test_grid();

    let entries = grid
        .entries
        .clone()
        .into_fill_bid()
        .expect("Failed to swap")
        .into_fill_bid()
        .expect("Failed to swap")
        .into_fill_bid()
        .expect("Failed to swap");

    let swapped = grid.clone().with_entries(entries).expect("Failed to swap");

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let swapped_candidate = swapped
        .clone()
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![swapped_candidate.clone(), fee_candidate.clone()],
    )
    .unwrap();

    prove_input(initial_box, tx, TestProver { secrets: vec![] }).expect("Failed to prove input");

    let swapped_box = ErgoBox::from_box_candidate(&swapped_candidate, TxId::zero(), 0).unwrap();

    let entries = swapped
        .entries
        .clone()
        .into_fill_ask()
        .expect("Failed to swap")
        .into_fill_ask()
        .expect("Failed to swap");

    let roundtripped = swapped.with_entries(entries).expect("Failed to swap");

    let roundtripped_candidate = roundtripped
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let roundtrip_tx = UnsignedTransaction::new_from_vec(
        vec![swapped_box.clone().into()],
        vec![],
        vec![roundtripped_candidate, fee_candidate],
    );

    MultiGridOrder::try_from(&swapped_box).expect("Failed to parse swapped box");

    prove_input(
        swapped_box,
        roundtrip_tx.unwrap(),
        TestProver { secrets: vec![] },
    )
    .expect("Failed to prove input");
}

#[test]
fn swap_wrong_tokens() {
    let (grid, _) = generate_test_grid();

    let entries = grid
        .entries
        .clone()
        .into_fill_bid()
        .expect("Failed to fill bid");

    let swapped = grid.clone().with_entries(entries).expect("Failed to swap");

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let mut swapped_candidate = swapped
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let wrong_token_id: TokenId = Digest::<32>([1u8; 32]).into();

    let wrong_token: Token = (
        wrong_token_id,
        swapped_candidate.tokens.unwrap().first().amount,
    )
        .into();

    swapped_candidate.tokens = Some(vec![wrong_token].try_into().unwrap());

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![swapped_candidate, fee_candidate],
    )
    .unwrap();

    let result = prove_input(initial_box, tx, TestProver { secrets: vec![] });

    assert!(result.is_err());
}

#[test]
fn wrong_output_index() {
    let (grid, _) = generate_test_grid();

    let entries = grid
        .entries
        .clone()
        .into_fill_bid()
        .expect("Failed to fill bid");

    let swapped = grid.clone().with_entries(entries).expect("Failed to swap");

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let swapped_candidate = swapped
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let fee_candidate = create_fee_candidate(MAX_FEE.try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![fee_candidate, swapped_candidate],
    )
    .unwrap();

    let result = prove_input(initial_box, tx, TestProver { secrets: vec![] });

    assert!(result.is_err());
}

#[test]
fn wrong_fee() {
    let (grid, _) = generate_test_grid();

    let entries = grid
        .entries
        .clone()
        .into_fill_bid()
        .expect("Failed to fill bid");

    let swapped = grid.clone().with_entries(entries).expect("Failed to swap");

    let box_candidate = grid
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let initial_box = ErgoBox::from_box_candidate(&box_candidate, TxId::zero(), 0).unwrap();

    let swapped_candidate = swapped
        .into_box_candidate(0)
        .expect("Failed to create box candidate");

    let fee_candidate = create_fee_candidate((MAX_FEE - 1).try_into().unwrap());

    let tx = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![swapped_candidate.clone(), fee_candidate],
    )
    .unwrap();

    prove_input(initial_box.clone(), tx, TestProver { secrets: vec![] })
        .expect_err("Should fail fee not equal");

    let tx2 = UnsignedTransaction::new_from_vec(
        vec![initial_box.clone().into()],
        vec![],
        vec![swapped_candidate],
    );

    prove_input(initial_box, tx2.unwrap(), TestProver { secrets: vec![] })
        .expect_err("Should fail no fee candidate");
}
