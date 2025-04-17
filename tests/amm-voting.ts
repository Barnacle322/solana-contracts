import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { SolanaContracts } from "../target/types/solana_contracts";
import { Keypair, PublicKey, SystemProgram, Connection } from "@solana/web3.js";
import { 
  TOKEN_PROGRAM_ID, 
  createMint, 
  getOrCreateAssociatedTokenAccount,
  mintTo,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAccount
} from "@solana/spl-token";
import { expect } from "chai";

describe("amm_voting", () => {
  // Configure the client to use the local cluster
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Configure the connection to use a confirmed commitment level
  const connection = new Connection("http://localhost:8899", "confirmed");

  const program = anchor.workspace.SolanaContracts as Program<SolanaContracts>;
  
  // Test accounts
  const admin = provider.wallet.payer;  // Use the provider's wallet to avoid airdrop issues
  const user1 = Keypair.generate();
  const user2 = Keypair.generate();
  
  // Token accounts
  let mint: PublicKey;
  let adminTokenAccount: PublicKey;
  let user1TokenAccount: PublicKey;
  let user2TokenAccount: PublicKey;
  let feeVault: PublicKey;
  
  // Poll accounts
  let pollKeypair = Keypair.generate();
  let poll: PublicKey;
  let vote1Keypair = Keypair.generate();
  let vote2Keypair = Keypair.generate();
  let poolVaultAccount: PublicKey;
  let poolAuthority: PublicKey;
  let poolAuthorityBump: number;

  // NFT mock data
  const nft1 = Keypair.generate().publicKey;
  const nft2 = Keypair.generate().publicKey;

  before(async () => {
    // Airdrop SOL to test accounts
    try {
      await provider.connection.confirmTransaction(
        await provider.connection.requestAirdrop(user1.publicKey, 1000000000),
        "confirmed"
      );
      await provider.connection.confirmTransaction(
        await provider.connection.requestAirdrop(user2.publicKey, 1000000000),
        "confirmed"
      );
    } catch (e) {
      console.log("Airdrop failed, continuing with test...");
      // Continue anyway as the test might work with existing funds
    }

    try {
      // Create test token (represents USDC or similar)
      mint = await createMint(
        provider.connection,
        admin,
        admin.publicKey,
        null,
        6 // 6 decimals like USDC
      );

      // Create token accounts using associated token accounts
      adminTokenAccount = (await getOrCreateAssociatedTokenAccount(
        provider.connection,
        admin,
        mint,
        admin.publicKey
      )).address;

      user1TokenAccount = (await getOrCreateAssociatedTokenAccount(
        provider.connection,
        admin, // payer
        mint,
        user1.publicKey
      )).address;

      user2TokenAccount = (await getOrCreateAssociatedTokenAccount(
        provider.connection,
        admin, // payer
        mint,
        user2.publicKey
      )).address;

      // Create fee vault (also using ATA)
      feeVault = (await getOrCreateAssociatedTokenAccount(
        provider.connection,
        admin,
        mint,
        admin.publicKey
      )).address;

      // Mint tokens to users
      await mintTo(
        provider.connection,
        admin,
        mint,
        user1TokenAccount,
        admin.publicKey,
        1000000000 // 1000 tokens
      );

      await mintTo(
        provider.connection,
        admin,
        mint,
        user2TokenAccount,
        admin.publicKey,
        1000000000 // 1000 tokens
      );

      // Find PDA for pool authority
      const [poolAuthorityPDA, bump] = await PublicKey.findProgramAddress(
        [
          Buffer.from("pool"),
          pollKeypair.publicKey.toBuffer(),
        ],
        program.programId
      );
      poolAuthority = poolAuthorityPDA;
      poolAuthorityBump = bump;

      // Create pool vault using ATA
      poolVaultAccount = (await getOrCreateAssociatedTokenAccount(
        provider.connection,
        admin,
        mint,
        poolAuthority,
        true // allowOwnerOffCurve: true for PDA
      )).address;
    } catch (error) {
      console.error("Setup error:", error);
      throw error;
    }
  });

  it("Creates a poll", async () => {
    // Generate title as bytes - make sure it's a Buffer of correct length
    const title = "Which NFT will be worth more?";
    const titleBytes = Buffer.from(title);
    
    // Get current timestamp + 1 day
    const now = Math.floor(Date.now() / 1000);
    const closesAt = now + 86400; // 1 day from now
    
    // Initial shares
    const initialNft1Shares = new anchor.BN(1000000000);
    const initialNft2Shares = new anchor.BN(1000000000);
    
    try {
      await program.methods
        .createPoll(
          titleBytes,
          new anchor.BN(closesAt),
          nft1,
          nft2,
          initialNft1Shares,
          initialNft2Shares
        )
        .accounts({
          poll: pollKeypair.publicKey,
          authority: admin.publicKey,
          tokenMint: mint,
        })
        .signers([admin, pollKeypair])
        .rpc();
      
      // Verify poll state
      const pollAccount = await program.account.poll.fetch(pollKeypair.publicKey);
      expect(pollAccount.authority.toString()).to.equal(admin.publicKey.toString());
      expect(pollAccount.nft1.toString()).to.equal(nft1.toString());
      expect(pollAccount.nft2.toString()).to.equal(nft2.toString());
      expect(pollAccount.nft1Shares.toString()).to.equal(initialNft1Shares.toString());
      expect(pollAccount.nft2Shares.toString()).to.equal(initialNft2Shares.toString());
      expect(pollAccount.k.toString()).to.equal(initialNft1Shares.mul(initialNft2Shares).toString());
      expect(pollAccount.status).to.deep.equal({ active: {} });
      expect(pollAccount.tokenMint.toString()).to.equal(mint.toString());
      expect(Buffer.from(pollAccount.title).toString().trim()).to.equal(title);
      
      poll = pollKeypair.publicKey;
    } catch (error) {
      console.error("Error creating poll:", error);
      throw error;
    }
  });

  it("User1 votes on NFT1", async () => {
    // Vote for NFT1 with 100 tokens
    const amount = new anchor.BN(100000000); // 100 tokens with 6 decimals
    
    try {
      if (!poll) {
        throw new Error("Poll account is not defined");
      }
      
      await program.methods
        .vote(1, amount)
        .accounts({
          poll: pollKeypair.publicKey,
          vote: vote1Keypair.publicKey,
          user: user1.publicKey,
          userTokenAccount: user1TokenAccount,
          poolVault: poolVaultAccount,
          feeVault: feeVault,
        })
        .signers([user1, vote1Keypair])
        .rpc();
      
      // Verify vote state
      const voteAccount = await program.account.vote.fetch(vote1Keypair.publicKey);
      expect(voteAccount.user.toString()).to.equal(user1.publicKey.toString());
      expect(voteAccount.votedForNft).to.equal(1);
      expect(voteAccount.claimed).to.equal(false);
      
      // Verify poll state updated
      const pollAccount = await program.account.poll.fetch(pollKeypair.publicKey);
      expect(pollAccount.nft1Shares.toString()).not.to.equal("10000"); // Should have changed
      
      // Verify tokens transferred
      const poolVaultInfo = await getAccount(provider.connection, poolVaultAccount);
      const feeVaultInfo = await getAccount(provider.connection, feeVault);
      
      // Fee should be 3% of 100 = 3 tokens, so vault should have 97 tokens
      expect(Number(poolVaultInfo.amount)).to.be.greaterThan(0);
      expect(Number(feeVaultInfo.amount)).to.be.greaterThan(0);
    } catch (error) {
      console.error("Error voting:", error);
      throw error;
    }
  });

  it("User2 votes on NFT2", async () => {
    // Vote for NFT2 with 200 tokens
    const amount = new anchor.BN(200000000); // 200 tokens with 6 decimals
    
    try {
      await program.methods
        .vote(2, amount)
        .accounts({
          poll: pollKeypair.publicKey,
          vote: vote2Keypair.publicKey,
          user: user2.publicKey,
          userTokenAccount: user2TokenAccount,
          poolVault: poolVaultAccount,
          feeVault: feeVault,
        })
        .signers([user2, vote2Keypair])
        .rpc();
      
      // Verify vote state
      const voteAccount = await program.account.vote.fetch(vote2Keypair.publicKey);
      expect(voteAccount.user.toString()).to.equal(user2.publicKey.toString());
      expect(voteAccount.votedForNft).to.equal(2);
      expect(voteAccount.claimed).to.equal(false);
    } catch (error) {
      console.error("Error voting:", error);
      throw error;
    }
  });

  it("Resolves the poll", async () => {
    try {
      await program.methods
        .resolvePoll(nft1) // NFT1 wins
        .accounts({
          poll: pollKeypair.publicKey,
          authority: admin.publicKey,
          admin: admin.publicKey, // Admin is the same as authority in this test
        })
        .signers([admin])
        .rpc();
      
      // Verify poll state
      const pollAccount = await program.account.poll.fetch(pollKeypair.publicKey);
      expect(pollAccount.status).to.deep.equal({ resolved: {} });
      expect(pollAccount.winningNft.toString()).to.equal(nft1.toString());
    } catch (error) {
      console.error("Error resolving poll:", error);
      throw error;
    }
  });

  it("User1 claims winnings", async () => {
    // User1 bet on NFT1 which won, so they should be able to claim
    try {
      await program.methods
        .claimWinnings()
        .accounts({
          poll: pollKeypair.publicKey,
          vote: vote1Keypair.publicKey,
          user: user1.publicKey,
          userTokenAccount: user1TokenAccount,
          poolVault: poolVaultAccount
        })
        .signers([user1])
        .rpc();
      
      // Verify vote marked as claimed
      const voteAccount = await program.account.vote.fetch(vote1Keypair.publicKey);
      expect(voteAccount.claimed).to.equal(true);
      
      // Verify tokens transferred to user
      const userTokenInfo = await getAccount(provider.connection, user1TokenAccount);
      expect(Number(userTokenInfo.amount)).to.be.greaterThan(0);
    } catch (error) {
      console.error("Error claiming winnings:", error);
      throw error;
    }
  });

  it("Prevents user2 from claiming (they bet on the wrong NFT)", async () => {
    // User2 bet on NFT2 which lost, so they should not be able to claim
    try {
      await program.methods
        .claimWinnings()
        .accounts({
          poll: pollKeypair.publicKey,
          vote: vote2Keypair.publicKey,
          user: user2.publicKey,
          userTokenAccount: user2TokenAccount,
          poolVault: poolVaultAccount
        })
        .signers([user2])
        .rpc();
      
      // This should fail, so we shouldn't reach here
      expect.fail("Should not be able to claim winnings for losing bet");
    } catch (error) {
      // Expected error
      expect(error.toString()).to.include("Vote did not win");
    }
  });

  it("Prevents double-claiming", async () => {
    // User1 already claimed, so they should not be able to claim again
    try {
      await program.methods
        .claimWinnings()
        .accounts({
          poll: pollKeypair.publicKey,
          vote: vote1Keypair.publicKey,
          user: user1.publicKey,
          userTokenAccount: user1TokenAccount,
          poolVault: poolVaultAccount
        })
        .signers([user1])
        .rpc();
      
      // This should fail, so we shouldn't reach here
      expect.fail("Should not be able to claim winnings twice");
    } catch (error) {
      // Expected error
      expect(error.toString()).to.include("Winnings already claimed");
    }
  });

  it("Prevents unauthorized users from resolving", async () => {
    // Create a new poll to test with
    const newPollKeypair = Keypair.generate();
    const title = "Another test poll";
    const titleBytes = Buffer.from(title);

    const now = Math.floor(Date.now() / 1000);
    const closesAt = now + 86400;
    const initialShares = new anchor.BN(1000000000);
    
    await program.methods
      .createPoll(
        titleBytes,
        new anchor.BN(closesAt),
        nft1,
        nft2,
        initialShares,
        initialShares
      )
      .accounts({
        poll: newPollKeypair.publicKey,
        authority: admin.publicKey,
        tokenMint: mint,
      })
      .signers([admin, newPollKeypair])
      .rpc();
    
    // User1 tries to resolve the poll (should fail)
    try {
      await program.methods
        .resolvePoll(nft1)
        .accounts({
          poll: newPollKeypair.publicKey,
          authority: user1.publicKey,
          admin: admin.publicKey,
        })
        .signers([user1])
        .rpc();
      
      // This should fail, so we shouldn't reach here
      expect.fail("Non-admin should not be able to resolve poll");
    } catch (error) {
      // Expected error
      expect(error.toString()).to.include("Unauthorized");
    }
  });
});