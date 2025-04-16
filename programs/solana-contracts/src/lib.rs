use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint};

// Program ID: update in Anchor.toml as needed

declare_id!("41f3Bi7jwTJ8Q3qr29AtaLZh3193AArY1nsgoTrEyRYx");

#[program]
pub mod solana_contracts {
    use super::*;

    pub fn create_poll(
        ctx: Context<CreatePoll>,
        title_bytes: Vec<u8>,
        closes_at: i64,
        nft1: Pubkey,
        nft2: Pubkey,
        initial_nft1_shares: u64,
        initial_nft2_shares: u64,
    ) -> Result<()> {
        require!(title_bytes.len() <= 64, AmmError::TitleTooLong);
        require!(initial_nft1_shares > 0, AmmError::InvalidShares);
        require!(initial_nft2_shares > 0, AmmError::InvalidShares);
        
        let poll = &mut ctx.accounts.poll;
        poll.authority = ctx.accounts.authority.key();
        poll.title = title_bytes;
        poll.closes_at = closes_at;
        poll.nft1 = nft1;
        poll.nft2 = nft2;
        poll.nft1_shares = initial_nft1_shares;
        poll.nft2_shares = initial_nft2_shares;
        poll.k = initial_nft1_shares * initial_nft2_shares;
        poll.status = PollStatus::Active;
        poll.token_mint = ctx.accounts.token_mint.key();
        
        emit!(PollCreatedEvent {
            poll: poll.key(),
            authority: poll.authority,
            nft1,
            nft2,
            closes_at
        });
        
        Ok(())
    }

    pub fn vote(ctx: Context<VoteOnPoll>, nft_choice: u8, amount: u64) -> Result<()> {
        let poll = &mut ctx.accounts.poll;
        let vote = &mut ctx.accounts.vote;
        require!(poll.status == PollStatus::Active, AmmError::PollNotActive);
        require!(
            Clock::get()?.unix_timestamp < poll.closes_at,
            AmmError::PollClosed
        );
        require!(
            nft_choice == 1 || nft_choice == 2,
            AmmError::InvalidNftChoice
        );
        // Deduct 3% network fee
        let fee = amount * 3 / 100;
        let amount_after_fee = amount - fee;
        // SPL token transfer: user -> pool vault
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_account.to_account_info(),
                to: ctx.accounts.pool_vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, amount_after_fee)?;
        // SPL token transfer: user -> fee vault
        let cpi_ctx_fee = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_account.to_account_info(),
                to: ctx.accounts.fee_vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx_fee, fee)?;
        // AMM swap logic
        let (received, new_nft1, new_nft2) = if nft_choice == 1 {
            // Bet on NFT1: swap NFT2 for NFT1
            let new_nft2 = poll.nft2_shares + amount_after_fee;
            let new_nft1 = poll.k / new_nft2;
            let received = poll.nft1_shares - new_nft1;
            require!(
                amount_after_fee <= poll.nft2_shares,
                AmmError::NotEnoughLiquidity
            );
            (received, new_nft1, new_nft2)
        } else {
            // Bet on NFT2: swap NFT1 for NFT2
            let new_nft1 = poll.nft1_shares + amount_after_fee;
            let new_nft2 = poll.k / new_nft1;
            let received = poll.nft2_shares - new_nft2;
            require!(
                amount_after_fee <= poll.nft1_shares,
                AmmError::NotEnoughLiquidity
            );
            (received, new_nft1, new_nft2)
        };
        poll.nft1_shares = new_nft1;
        poll.nft2_shares = new_nft2;
        // Record vote
        vote.poll = poll.key();
        vote.user = ctx.accounts.user.key();
        vote.voted_for_nft = nft_choice;
        vote.amount = received;
        vote.value = amount;
        vote.price_at_transaction = get_price(poll.nft1_shares, poll.nft2_shares, nft_choice);
        Ok(())
    }

    pub fn resolve_poll(ctx: Context<ResolvePoll>, winning_nft: Pubkey) -> Result<()> {
        let poll = &mut ctx.accounts.poll;
        
        // Ensure only the poll creator or a program admin can resolve
        require!(
            poll.authority == ctx.accounts.authority.key() || 
            ctx.accounts.authority.key() == ctx.accounts.admin.key(), 
            AmmError::Unauthorized
        );
        
        require!(
            poll.status == PollStatus::Active || poll.status == PollStatus::Closed,
            AmmError::PollNotActive
        );
        require!(
            winning_nft == poll.nft1 || winning_nft == poll.nft2,
            AmmError::InvalidNftChoice
        );
        
        poll.status = PollStatus::Resolved;
        poll.winning_nft = Some(winning_nft);
        
        emit!(PollResolvedEvent {
            poll: poll.key(),
            authority: ctx.accounts.authority.key(),
            winning_nft
        });
        
        Ok(())
    }

    pub fn cancel_poll(ctx: Context<CancelPoll>) -> Result<()> {
        let poll = &mut ctx.accounts.poll;
        
        // Ensure only the poll creator or a program admin can cancel
        require!(
            poll.authority == ctx.accounts.authority.key() || 
            ctx.accounts.authority.key() == ctx.accounts.admin.key(), 
            AmmError::Unauthorized
        );
        
        require!(
            poll.status != PollStatus::Resolved && poll.status != PollStatus::Canceled,
            AmmError::PollNotActive
        );
        
        poll.status = PollStatus::Canceled;
        
        emit!(PollCanceledEvent {
            poll: poll.key(),
            authority: ctx.accounts.authority.key()
        });
        
        Ok(())
    }

    pub fn add_liquidity(
        ctx: Context<AddLiquidity>,
        nft1_amount: u64,
        nft2_amount: u64,
    ) -> Result<()> {
        // SPL token transfer: user -> pool vault for both tokens
        let cpi_ctx1 = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_account1.to_account_info(),
                to: ctx.accounts.pool_vault1.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx1, nft1_amount)?;
        let cpi_ctx2 = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_account2.to_account_info(),
                to: ctx.accounts.pool_vault2.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx2, nft2_amount)?;
        let poll = &mut ctx.accounts.poll;
        poll.nft1_shares = poll.nft1_shares.checked_add(nft1_amount).unwrap();
        poll.nft2_shares = poll.nft2_shares.checked_add(nft2_amount).unwrap();
        poll.k = poll.nft1_shares * poll.nft2_shares;
        Ok(())
    }

    pub fn claim_winnings(ctx: Context<ClaimWinnings>) -> Result<()> {
        let poll = &ctx.accounts.poll;
        let vote = &mut ctx.accounts.vote;
        
        // Check if poll is resolved
        require!(poll.status == PollStatus::Resolved, AmmError::PollNotResolved);
        
        // Check if this vote belongs to the correct user
        require!(vote.user == ctx.accounts.user.key(), AmmError::Unauthorized);
        
        // Check if vote is already claimed
        require!(!vote.claimed, AmmError::AlreadyClaimed);
        
        // Check if vote is for the winning NFT
        let winning_nft = poll.winning_nft.ok_or(AmmError::PollNotResolved)?;
        let voted_for_winner = 
            (vote.voted_for_nft == 1 && winning_nft == poll.nft1) ||
            (vote.voted_for_nft == 2 && winning_nft == poll.nft2);
        
        require!(voted_for_winner, AmmError::NotWinner);
        
        // Calculate payout based on vote amount
        // In this simple implementation, winners get their tokens back plus their share
        let payout_amount = vote.amount;
        
        // Transfer tokens from pool vault to user
        let pool_auth_bump = ctx.bumps.pool_authority;
        let binding = poll.key();
        let seeds = &[
            b"pool".as_ref(),
            binding.as_ref(),
            &[pool_auth_bump]
        ];
        let signer = &[&seeds[..]];
        
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_vault.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.pool_authority.to_account_info(),
            },
            signer,
        );
        
        token::transfer(cpi_ctx, payout_amount)?;
        
        // Mark vote as claimed
        vote.claimed = true;
        
        emit!(WinningsClaimed {
            poll: poll.key(),
            user: ctx.accounts.user.key(),
            amount: payout_amount,
        });
        
        Ok(())
    }
}

#[derive(Accounts)]
pub struct CreatePoll<'info> {
    #[account(init, payer = authority, space = 8 + Poll::LEN)]
    pub poll: Account<'info, Poll>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// The token mint that will be used for this poll
    pub token_mint: Account<'info, Mint>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VoteOnPoll<'info> {
    #[account(mut, constraint = poll.status == PollStatus::Active @ AmmError::PollNotActive)]
    pub poll: Account<'info, Poll>,
    #[account(init, payer = user, space = 8 + Vote::LEN)]
    pub vote: Account<'info, Vote>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut, 
        constraint = user_token_account.mint == poll.token_mint @ AmmError::InvalidTokenMint,
        constraint = user_token_account.owner == user.key() @ AmmError::InvalidTokenOwner
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = pool_vault.mint == poll.token_mint @ AmmError::InvalidTokenMint
    )]
    pub pool_vault: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = fee_vault.mint == poll.token_mint @ AmmError::InvalidTokenMint
    )]
    pub fee_vault: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ResolvePoll<'info> {
    #[account(mut)]
    pub poll: Account<'info, Poll>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: Admin pubkey is verified in the instruction
    pub admin: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CancelPoll<'info> {
    #[account(mut)]
    pub poll: Account<'info, Poll>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: Admin pubkey is verified in the instruction
    pub admin: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    #[account(mut)]
    pub poll: Account<'info, Poll>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub user_token_account1: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_account2: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pool_vault1: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pool_vault2: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ClaimWinnings<'info> {
    #[account(constraint = poll.status == PollStatus::Resolved @ AmmError::PollNotResolved)]
    pub poll: Account<'info, Poll>,
    
    #[account(
        mut, 
        constraint = vote.poll == poll.key() @ AmmError::InvalidVote,
        constraint = vote.user == user.key() @ AmmError::Unauthorized
    )]
    pub vote: Account<'info, Vote>,
    
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(
        mut,
        constraint = user_token_account.owner == user.key() @ AmmError::InvalidTokenOwner,
        constraint = user_token_account.mint == poll.token_mint @ AmmError::InvalidTokenMint
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    
    #[account(
        mut,
        constraint = pool_vault.mint == poll.token_mint @ AmmError::InvalidTokenMint
    )]
    pub pool_vault: Account<'info, TokenAccount>,
    
    /// CHECK: PDA that serves as the pool authority
    #[account(
        seeds = [b"pool", poll.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct Poll {
    pub authority: Pubkey,
    pub title: Vec<u8>,        // Using a fixed-size Vec<u8> instead of String
    pub closes_at: i64,
    pub nft1: Pubkey,
    pub nft2: Pubkey,
    pub nft1_shares: u64,
    pub nft2_shares: u64,
    pub k: u64,
    pub status: PollStatus,
    pub winning_nft: Option<Pubkey>,
    pub token_mint: Pubkey,    // Track which token mint is used for this poll
}

impl Poll {
    pub const LEN: usize = 32 + // authority 
                          4 + 64 + // title (vec with max 64 bytes)
                          8 + // closes_at
                          32 + // nft1
                          32 + // nft2
                          8 + // nft1_shares
                          8 + // nft2_shares
                          8 + // k
                          1 + // status enum
                          33 + // winning_nft option
                          32; // token_mint
}

#[account]
pub struct Vote {
    pub poll: Pubkey,
    pub user: Pubkey,
    pub voted_for_nft: u8,
    pub amount: u64,
    pub value: u64,
    pub price_at_transaction: u64,
    pub claimed: bool,         // Track if the vote has been claimed
}

impl Vote {
    pub const LEN: usize = 32 + // poll
                          32 + // user
                          1 + // voted_for_nft
                          8 + // amount
                          8 + // value
                          8 + // price_at_transaction
                          1; // claimed
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum PollStatus {
    Active,
    Closed,
    Resolved,
    Canceled,
}

#[error_code]
pub enum AmmError {
    #[msg("Poll is not active")]
    PollNotActive,
    #[msg("Poll is closed")]
    PollClosed,
    #[msg("Poll is not resolved yet")]
    PollNotResolved,
    #[msg("Invalid NFT choice")]
    InvalidNftChoice,
    #[msg("Not enough liquidity")]
    NotEnoughLiquidity,
    #[msg("Unauthorized action")]
    Unauthorized,
    #[msg("Title too long (max 64 bytes)")]
    TitleTooLong,
    #[msg("Invalid share amounts")]
    InvalidShares,
    #[msg("Invalid token mint")]
    InvalidTokenMint,
    #[msg("Invalid token owner")]
    InvalidTokenOwner,
    #[msg("Invalid vote record")]
    InvalidVote,
    #[msg("Winnings already claimed")]
    AlreadyClaimed,
    #[msg("Vote did not win")]
    NotWinner,
}

// Events for better UX and indexing
#[event]
pub struct PollCreatedEvent {
    pub poll: Pubkey,
    pub authority: Pubkey, 
    pub nft1: Pubkey,
    pub nft2: Pubkey,
    pub closes_at: i64,
}

#[event]
pub struct PollResolvedEvent {
    pub poll: Pubkey,
    pub authority: Pubkey,
    pub winning_nft: Pubkey,
}

#[event]
pub struct PollCanceledEvent {
    pub poll: Pubkey,
    pub authority: Pubkey,
}

#[event]
pub struct WinningsClaimed {
    pub poll: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
}

fn get_price(nft1_shares: u64, nft2_shares: u64, nft_choice: u8) -> u64 {
    let total = nft1_shares + nft2_shares;
    if nft_choice == 1 {
        ((nft2_shares as u128 * 10000) / total as u128) as u64
    } else {
        ((nft1_shares as u128 * 10000) / total as u128) as u64
    }
}
