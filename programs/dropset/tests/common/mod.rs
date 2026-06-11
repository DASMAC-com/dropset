use anchor_lang_v2::{bytemuck, Discriminator};
use anchor_v2_testing::{
    Keypair, LiteSVM, Message, Signer, VersionedMessage, VersionedTransaction,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::{instruction as loader_v3, state::UpgradeableLoaderState};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;

pub use dropset::ID as PROGRAM_ID;

/// Shared market bootstrap + per-instruction ix-builders. See
/// [`fixture::Fixture`].
pub mod fixture;

const PROGRAM_SO_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/deploy/dropset.so"
);
const PROGRAM_KEYPAIR_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/deploy/dropset-keypair.json"
);

/// Fits comfortably under the per-txn size limit.
const WRITE_CHUNK: usize = 900;

/// Buffer rent + fees across `create_buffer` + `Write`s + `deploy`.
pub const PAYER_FUNDING_LAMPORTS: u64 = 100 * LAMPORTS_PER_SOL;
/// Covers txn fees only.
pub const SIGNER_FUNDING_LAMPORTS: u64 = LAMPORTS_PER_SOL;

/// Real upgradeable-loader deploy (`create_buffer` → chunked `Write`s →
/// `DeployWithMaxDataLen`) with `authority` as the upgrade authority.
pub fn deploy_with_authority(authority: &Keypair) -> LiteSVM {
    let mut svm = anchor_v2_testing::svm();

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), PAYER_FUNDING_LAMPORTS)
        .unwrap();
    svm.airdrop(&authority.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();

    let program_kp = solana_keypair::read_keypair_file(PROGRAM_KEYPAIR_PATH)
        .expect("program keypair (run `anchor keys sync && anchor build`)");
    assert_eq!(program_kp.pubkey(), PROGRAM_ID);
    let program_so = std::fs::read(PROGRAM_SO_PATH).expect("program .so (run `anchor build`)");
    let buffer_kp = Keypair::new();

    let buffer_lamports = svm.minimum_balance_for_rent_exemption(
        UpgradeableLoaderState::size_of_buffer(program_so.len()),
    );
    let create_buffer = loader_v3::create_buffer(
        &payer.pubkey(),
        &buffer_kp.pubkey(),
        &authority.pubkey(),
        buffer_lamports,
        program_so.len(),
    )
    .unwrap();
    send_signed(&mut svm, &[&payer, &buffer_kp], &create_buffer);

    for (i, chunk) in program_so.chunks(WRITE_CHUNK).enumerate() {
        let ixn = loader_v3::write(
            &buffer_kp.pubkey(),
            &authority.pubkey(),
            (i * WRITE_CHUNK) as u32,
            chunk.to_vec(),
        );
        send_signed(&mut svm, &[&payer, authority], &[ixn]);
    }

    let program_lamports =
        svm.minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_program());
    let deploy = loader_v3::deploy_with_max_program_len(
        &payer.pubkey(),
        &PROGRAM_ID,
        &buffer_kp.pubkey(),
        &authority.pubkey(),
        program_lamports,
        program_so.len(),
    )
    .unwrap();
    send_signed(&mut svm, &[&payer, &program_kp, authority], &deploy);

    svm
}

/// Send a single instruction signed by `signer` (also the fee payer).
/// Returns the debug-formatted runtime error on failure.
pub fn send_ixn(svm: &mut LiteSVM, signer: &Keypair, ixn: Instruction) -> Result<(), String> {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ixn], Some(&signer.pubkey()), &blockhash);
    let txn = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[signer]).unwrap();
    svm.send_transaction(txn)
        .map(|_| ())
        .map_err(|e| format!("{:?}", e.err))
}

/// Decode a `Slab<H, T>` account buffer into `(header, tail items)`.
/// Layout: `[disc:8][H][len:u32 LE][items...]`. Assumes the standard
/// 8-byte `#[account]` discriminator and `align_of::<T>() == 1` (true
/// for our `Address`/`Pod`-byte tails), so items start right after the
/// length prefix with no padding.
#[allow(dead_code)]
pub fn decode_slab<H, T>(data: &[u8]) -> (&H, &[T])
where
    H: bytemuck::Pod,
    T: bytemuck::Pod,
{
    const DISC: usize = 8;
    let header_end = DISC + core::mem::size_of::<H>();
    let header = bytemuck::from_bytes::<H>(&data[DISC..header_end]);
    let len = u32::from_le_bytes(data[header_end..header_end + 4].try_into().unwrap()) as usize;
    let items_start = header_end + 4;
    let items = bytemuck::cast_slice::<u8, T>(
        &data[items_start..items_start + len * core::mem::size_of::<T>()],
    );
    (header, items)
}

/// Assert a `send_ixn` failure string carries the program's custom error
/// code for `code`. Derives the expected `Custom(N)` from the program's
/// own `From<DropsetError>` mapping, so it tracks any change to the error
/// offset or variant order rather than hard-coding a number.
#[allow(dead_code)]
pub fn assert_program_error(err: &str, code: dropset::DropsetError) {
    let expected = format!("{:?}", anchor_lang_v2::Error::from(code));
    assert!(err.contains(&expected), "expected {expected}, got: {err}");
}

/// Assert a `send_ixn` failure carries `variant` of Solana's
/// `InstructionError` — e.g. `"IllegalOwner"`, `"InvalidAccountData"`,
/// `"IncorrectProgramId"`, `"InvalidSeeds"`. Used for negative tests
/// whose rejection comes from the runtime or a CPI rather than from a
/// `DropsetError`.
#[allow(dead_code)]
pub fn assert_instruction_error(err: &str, variant: &str) {
    assert!(
        err.contains(variant),
        "expected InstructionError containing {variant:?}, got: {err}"
    );
}

/// Assert an anchor `#[account]` buffer matches `expected` exactly: owned
/// by the program, prefixed with `T`'s discriminator, body bytes
/// bytemuck-equal to `expected`, total length equal to
/// `disc.len() + size_of::<T>()` (catches any padding / trailing bytes the
/// framework might inject).
#[allow(dead_code)]
pub fn assert_anchor_account_eq<T>(data: &[u8], owner: &Pubkey, expected: &T)
where
    T: Discriminator + bytemuck::Pod,
{
    let disc_len = T::DISCRIMINATOR.len();
    assert_eq!(owner, &PROGRAM_ID, "account not owned by program");
    assert_eq!(
        data.len(),
        disc_len + core::mem::size_of::<T>(),
        "account length != disc + size_of::<T>()"
    );
    let (disc, body) = data.split_at(disc_len);
    assert_eq!(disc, T::DISCRIMINATOR, "discriminator mismatch");
    assert_eq!(body, bytemuck::bytes_of(expected), "body bytes mismatch");
}

/// SPL Token program ID.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Token-2022 (Token Extensions) program ID.
pub const TOKEN_2022_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
/// Associated Token Account program ID.
pub const ATA_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

/// Derive the associated-token-account address for `(wallet, mint, token_program)`.
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ATA_PROGRAM_ID,
    )
    .0
}

/// SPL Token Mint account size (bytes).
const MINT_LEN: usize = 82;
/// SPL Token Account size (bytes).
#[allow(dead_code)]
const TOKEN_ACCOUNT_LEN: usize = 165;

/// Create an SPL Token mint with 6 decimals, owned by `authority`.
/// Returns the mint address.
pub fn create_spl_mint(svm: &mut LiteSVM, authority: &Keypair) -> Pubkey {
    let mint_kp = Keypair::new();
    let lamports = svm.minimum_balance_for_rent_exemption(MINT_LEN);

    // SystemProgram::CreateAccount instruction (index 0).
    let mut create_data = Vec::with_capacity(4 + 8 + 8 + 32);
    create_data.extend_from_slice(&0u32.to_le_bytes()); // instruction index
    create_data.extend_from_slice(&lamports.to_le_bytes());
    create_data.extend_from_slice(&(MINT_LEN as u64).to_le_bytes());
    create_data.extend_from_slice(&SPL_TOKEN_PROGRAM_ID.to_bytes());

    let create = Instruction::new_with_bytes(
        SYSTEM_PROGRAM_ID,
        &create_data,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(mint_kp.pubkey(), true),
        ],
    );

    // InitializeMint2 instruction data: [20u8, decimals, mint_authority(32), 0 (no freeze)]
    let mut mint_data = vec![20u8, 6]; // discriminator + 6 decimals
    mint_data.extend_from_slice(&authority.pubkey().to_bytes()); // mint authority
    mint_data.push(0); // COption::None for freeze authority

    let init_mint = Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &mint_data,
        vec![AccountMeta::new(mint_kp.pubkey(), false)],
    );

    send_signed(svm, &[authority, &mint_kp], &[create, init_mint]);
    mint_kp.pubkey()
}

/// Create a Token-2022 mint with 6 decimals, owned by `authority`.
/// Returns the mint address.
#[allow(dead_code)]
pub fn create_token2022_mint(svm: &mut LiteSVM, authority: &Keypair) -> Pubkey {
    let mint_kp = Keypair::new();
    let lamports = svm.minimum_balance_for_rent_exemption(MINT_LEN);

    let mut create_data = Vec::with_capacity(4 + 8 + 8 + 32);
    create_data.extend_from_slice(&0u32.to_le_bytes());
    create_data.extend_from_slice(&lamports.to_le_bytes());
    create_data.extend_from_slice(&(MINT_LEN as u64).to_le_bytes());
    create_data.extend_from_slice(&TOKEN_2022_PROGRAM_ID.to_bytes());

    let create = Instruction::new_with_bytes(
        SYSTEM_PROGRAM_ID,
        &create_data,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(mint_kp.pubkey(), true),
        ],
    );

    let mut mint_data = vec![20u8, 6];
    mint_data.extend_from_slice(&authority.pubkey().to_bytes());
    mint_data.push(0); // COption::None for freeze authority

    let init_mint = Instruction::new_with_bytes(
        TOKEN_2022_PROGRAM_ID,
        &mint_data,
        vec![AccountMeta::new(mint_kp.pubkey(), false)],
    );

    send_signed(svm, &[authority, &mint_kp], &[create, init_mint]);
    mint_kp.pubkey()
}

/// Create a Token-2022 token account (165 bytes) for the given mint.
/// Returns the token account address — useful for testing that the
/// program correctly rejects a token account passed as a mint.
#[allow(dead_code)]
pub fn create_token2022_token_account(
    svm: &mut LiteSVM,
    authority: &Keypair,
    mint: &Pubkey,
) -> Pubkey {
    let acct_kp = Keypair::new();
    let lamports = svm.minimum_balance_for_rent_exemption(TOKEN_ACCOUNT_LEN);

    let mut create_data = Vec::with_capacity(4 + 8 + 8 + 32);
    create_data.extend_from_slice(&0u32.to_le_bytes());
    create_data.extend_from_slice(&lamports.to_le_bytes());
    create_data.extend_from_slice(&(TOKEN_ACCOUNT_LEN as u64).to_le_bytes());
    create_data.extend_from_slice(&TOKEN_2022_PROGRAM_ID.to_bytes());

    let create = Instruction::new_with_bytes(
        SYSTEM_PROGRAM_ID,
        &create_data,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(acct_kp.pubkey(), true),
        ],
    );

    // InitializeAccount3: [18u8, owner_pubkey(32)]
    let mut init_data = vec![18u8];
    init_data.extend_from_slice(&authority.pubkey().to_bytes());

    let init_acct = Instruction::new_with_bytes(
        TOKEN_2022_PROGRAM_ID,
        &init_data,
        vec![
            AccountMeta::new(acct_kp.pubkey(), false),
            AccountMeta::new_readonly(*mint, false),
        ],
    );

    send_signed(svm, &[authority, &acct_kp], &[create, init_acct]);
    acct_kp.pubkey()
}

pub fn send_signed(svm: &mut LiteSVM, signers: &[&Keypair], instructions: &[Instruction]) {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(instructions, Some(&signers[0].pubkey()), &blockhash);
    let txn = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(txn)
        .map_err(|e| format!("setup txn failed: {:?}\nlogs: {:?}", e.err, e.meta.logs))
        .unwrap();
}

// ── Mock USDC + ATA helpers used by the `register_market` tests ─────

/// Mock-USDC mint decimals — matches real USDC.
#[allow(dead_code)]
pub const MOCK_USDC_DECIMALS: u8 = 6;

/// $1,000 in mock-USDC atoms (6 decimals): the open-market fee used by
/// the register-market test fixtures.
#[allow(dead_code)]
pub const REGISTER_MARKET_FEE_ATOMS: u64 = 1_000 * 1_000_000;

/// Create a USDC-shaped SPL Token mint with [`MOCK_USDC_DECIMALS`].
///
/// Returns the mint address; `authority` becomes the mint authority,
/// which lets tests mint mock-USDC to a payer before charging the
/// open-market fee.
#[allow(dead_code)]
pub fn create_mock_usdc_mint(svm: &mut LiteSVM, authority: &Keypair) -> Pubkey {
    // Real USDC is 6 decimals under the classic SPL Token program; the
    // existing helper already builds that exact shape, so reuse it
    // rather than duplicating the InitializeMint2 plumbing.
    create_spl_mint(svm, authority)
}

/// Mint `amount` atoms of `mint` to `destination_ata` under the SPL
/// Token program. `mint_authority` must be a signer with mint authority
/// on `mint`.
#[allow(dead_code)]
pub fn mint_to(
    svm: &mut LiteSVM,
    mint_authority: &Keypair,
    mint: &Pubkey,
    destination_ata: &Pubkey,
    amount: u64,
) {
    // SPL Token `MintTo` ix (discriminator 7): [7u8, amount(8 bytes LE)].
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination_ata, false),
            AccountMeta::new_readonly(mint_authority.pubkey(), true),
        ],
    );
    send_signed(svm, &[mint_authority], &[ix]);
}

/// Create the ATA for `(wallet, mint, token_program)` via the ATA
/// program, funded by `payer`. Returns the ATA address. Works for both
/// SPL Token and Token-2022 (the ATA program selects on the supplied
/// program id).
#[allow(dead_code)]
pub fn create_associated_token_account(
    svm: &mut LiteSVM,
    payer: &Keypair,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Pubkey {
    let ata = associated_token_address(wallet, mint, token_program);
    // ATA `Create` (discriminator 0) takes no extra data; account list
    // matches the ATA program's documented order.
    let ix = Instruction::new_with_bytes(
        ATA_PROGRAM_ID,
        &[0u8],
        vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
    );
    send_signed(svm, &[payer], &[ix]);
    ata
}
