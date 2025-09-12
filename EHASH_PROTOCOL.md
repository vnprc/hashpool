## Protocol Flow

1. Before mining begins, the pool must authenticate with the mint as an **ehash issuer**.
2. To support multiple mining pools, the mint must support **roles**.  
   - Achievable via string matching: each HASH currency unit includes the pool's name.  
   - Pools may only create quotes for their own currency unit.
3. When a share is received by the pool, it submits an **authenticated quote creation request** to the mint.
4. If the pool creates block templates → mint quote is created in **PAID** status.  
   *(Future work: if the miner submits block templates → pool creates quotes **UNPAID**, changing to **PAID** upon template validation).*
5. When the pool receives a mining reward, it **pegs in with the mint** and receives ecash to the pool wallet  
   - For a solo pool: delayed **100 blocks**.
6. At the same time, the pool sends an **authenticated request** to the mint to create a new asset for the new mining epoch.  
   - The pool stops creating quotes for the closed epoch.
7. When the pool receives ecash from the mint, it enables **ehash swaps** from the closed epoch.
8. Miners and pool use **HTLC locking** to perform ehash → ecash swaps.
9. After a predefined **TTL**, the pool closes ehash swaps at the mint (authenticated request) and requests a **proof of liabilities (PoL)** report.
10. Pool generates a **proof of shares** report, reconciles with the PoL, and publishes both.
11. **Open question:** does the pool need to explicitly "destroy" collected ehash tokens?  
    - If so, should this be an authenticated request to the mint?

---

### Required New Mint Capabilities
- **Authenticated quote creation**  
- **Authenticated asset creation**
