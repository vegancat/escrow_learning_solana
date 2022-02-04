use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::{IsInitialized, Pack},
    pubkey::Pubkey,
    sysvar::{rent::Rent, Sysvar},
};

use spl_token::state::Account as TokenAccount;

use crate::{error::EscrowError, instruction::EscrowInstruction, state::Escrow};
pub struct Processor;
impl Processor {
    pub fn process(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        instruction_data: &[u8],
    ) -> ProgramResult {
        let instruction = EscrowInstruction::unpack(instruction_data)?;

        match instruction {
            EscrowInstruction::InitEscrow { amount } => {
                msg!("Instruction: InitEscrow");
                Self::process_init_escrow(accounts, amount, program_id)
            }

            EscrowInstruction::Exchange { amount } => {
                msg!("Instruction: Exchange");
                Self::process_trade(accounts, amount, program_id)
            }
        }
    }

    fn process_init_escrow(
        accounts: &[AccountInfo],
        amount: u64,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let initializer = next_account_info(account_info_iter)?;

        if !initializer.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let temp_token_account = next_account_info(account_info_iter)?;

        let token_to_receive_account = next_account_info(account_info_iter)?;
        if *token_to_receive_account.owner != spl_token::id() {
            return Err(ProgramError::IncorrectProgramId);
        }

        let escrow_account = next_account_info(account_info_iter)?;
        let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;

        if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
            return Err(EscrowError::NotRentExempt.into());
        }

        let mut escrow_info = Escrow::unpack_unchecked(&escrow_account.try_borrow_data()?)?;
        if escrow_info.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        escrow_info.is_initialized = true;
        escrow_info.initializer_pubkey = *initializer.key;
        escrow_info.temp_token_account_pubkey = *temp_token_account.key;
        escrow_info.initializer_token_to_receive_account_pubkey = *token_to_receive_account.key;
        escrow_info.expected_amount = amount;

        Escrow::pack(escrow_info, &mut escrow_account.try_borrow_mut_data()?)?;

        let (pda, _bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

        let token_program = next_account_info(account_info_iter)?;
        let owner_change_ix = spl_token::instruction::set_authority(
            token_program.key,
            temp_token_account.key,
            Some(&pda),
            spl_token::instruction::AuthorityType::AccountOwner,
            initializer.key,
            &[&initializer.key],
        )?;

        msg!("Calling the token program to transfer token account ownership...");
        invoke(
            &owner_change_ix,
            &[
                temp_token_account.clone(),
                initializer.clone(),
                token_program.clone(),
            ],
        )?;

        Ok(())
    }

    fn process_trade(
        accounts: &[AccountInfo],
        expected_amount: u64,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let accounts_info_iter = &mut accounts.iter();
        let trade_taker_account = next_account_info(accounts_info_iter)?;

        // checking if this account is the signer
        if !trade_taker_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        // getting the amount of Y tokens in takers account
        let taker_token_to_send_account = next_account_info(accounts_info_iter)?;

        let taker_token_to_recieve_account = next_account_info(accounts_info_iter)?;

        let pdas_temp_token_account = next_account_info(accounts_info_iter)?;
        let pdas_temp_token_account_info =
            TokenAccount::unpack(&pdas_temp_token_account.try_borrow_data()?)?;

        if pdas_temp_token_account_info.amount != expected_amount {
            return Err(EscrowError::ExpectedAmountMissmatch.into());
        }

        let initializer_account = next_account_info(accounts_info_iter)?;
        let initializer_token_to_recieve_account = next_account_info(accounts_info_iter)?;
        let escrow_account = next_account_info(accounts_info_iter)?;

        let escrow_info = Escrow::unpack(&escrow_account.try_borrow_data()?)?;
        let taker_token_to_send_info =
            TokenAccount::unpack(&taker_token_to_send_account.try_borrow_data()?)?;

        if taker_token_to_send_info.amount < escrow_info.expected_amount {
            return Err(EscrowError::ExpectedAmountMissmatch.into());
        }

        if escrow_info.initializer_pubkey != *initializer_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_info.temp_token_account_pubkey != *pdas_temp_token_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_info.initializer_token_to_receive_account_pubkey
            != *initializer_token_to_recieve_account.key
        {
            return Err(ProgramError::InvalidAccountData);
        }

        let token_program = next_account_info(accounts_info_iter)?;
        let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], &program_id);

        let transfer_y_to_initializer_ix = spl_token::instruction::transfer(
            token_program.key,
            taker_token_to_send_account.key,
            initializer_token_to_recieve_account.key,
            trade_taker_account.key,
            &[&trade_taker_account.key],
            escrow_info.expected_amount,
        )?;

        // transfers y from taker to initializer
        invoke(
            &transfer_y_to_initializer_ix,
            &[
                token_program.clone(),
                taker_token_to_send_account.clone(),
                initializer_token_to_recieve_account.clone(),
                trade_taker_account.clone(),
            ],
        )?;

        // invoke(&transfer_x_to_trade_taker_ix, &[token_program, pdas_temp_token_account, taker_token_to_recieve_account, ])

        let pda_account = next_account_info(accounts_info_iter)?;

        let transfer_x_to_trade_taker_ix = spl_token::instruction::transfer(
            token_program.key,
            pdas_temp_token_account.key,
            taker_token_to_recieve_account.key,
            &pda,
            &[&pda],
            expected_amount,
        )?;

        msg!("Calling the token program to transfer tokens to the taker...");
        invoke_signed(
            &transfer_x_to_trade_taker_ix,
            &[
                token_program.clone(),
                pdas_temp_token_account.clone(),
                taker_token_to_recieve_account.clone(),
                pda_account.clone(),
            ],
            &[&[&b"escrow"[..], &[bump_seed]]],
        )?;

        let close_pdas_temp_account_ix = spl_token::instruction::close_account(
            token_program.key,
            pdas_temp_token_account.key,
            initializer_account.key,
            &pda,
            &[&pda],
        )?;

        msg!("Calling the token program to close the pda's temp account...");
        invoke_signed(
            &close_pdas_temp_account_ix,
            &[
                token_program.clone(),
                pdas_temp_token_account.clone(),
                initializer_account.clone(),
                pda_account.clone(),
            ],
            &[&[&b"escrow"[..], &[bump_seed]]],
        )?;

        msg!("Closing the escrow account...");
        **initializer_account.lamports.borrow_mut() = initializer_account
            .lamports()
            .checked_add(escrow_account.lamports())
            .ok_or(EscrowError::AmountOverFlow)?;

        **escrow_account.lamports.borrow_mut() = 0;
        *escrow_account.try_borrow_mut_data()? = &mut [];

        Ok(())
    }
}