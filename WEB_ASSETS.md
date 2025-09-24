# SVG Icon Consolidation Plan

## Current Problem Analysis

- Mining pickaxe SVG is duplicated **6 times** across 2 files:
  - `pool/src/lib/web.rs`: 2 occurrences (constant + CSS)
  - `translator/src/lib/web.rs`: 4 occurrences (3 different HTML pages + constant)
- Each instance is a 400+ character data URI that's hard to maintain
- Changes require updating multiple locations
- No shared access between pool and translator components

## Proposed Solution: Web Assets Utility Crate

### 1. Create New Shared Crate

```
roles/roles-utils/web-assets/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   └── icons.rs
```

### 2. Crate Structure

- **Package name**: `web_assets` 
- **Module**: `icons` containing SVG constants and helper functions
- **Exports**: 
  - SVG data URI constants
  - CSS generation functions
  - Inline SVG helper functions

### 3. Icons Module Design

```rust
// src/icons.rs
pub const MINING_ICON_SVG: &str = r#"data:image/svg+xml;charset=utf8,<svg>...</svg>"#;

pub fn mining_icon_css() -> &'static str {
    r#"
    .mining-icon::before {
        content: '';
        display: inline-block;
        width: 1.2em;
        height: 1.2em;
        vertical-align: middle;
        margin-right: 0.3em;
        background-image: url('{}');
        background-size: contain;
        background-repeat: no-repeat;
    }
    a:hover .mining-icon {
        text-shadow: 0 0 10px #00ff00;
    }
    a:hover .mining-icon::before {
        filter: drop-shadow(0 0 10px #00ff00);
    }"#.replace("{}", MINING_ICON_SVG)
}

pub fn inline_mining_icon_svg() -> &'static str {
    r#"<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">...</svg>"#
}
```

### 4. Integration Steps

#### Step 4.1: Create the crate
- Add `roles-utils/web-assets/` directory
- Create Cargo.toml with no external dependencies
- Add to workspace members in root Cargo.toml

#### Step 4.2: Update pool web.rs
- Add `web_assets` dependency to pool's Cargo.toml
- Replace hardcoded SVG with `use web_assets::icons::*`  
- Use `mining_icon_css()` function instead of hardcoded CSS
- Remove duplicate constants and helper functions

#### Step 4.3: Update translator web.rs
- Add `web_assets` dependency to translator's Cargo.toml
- Replace all 4 SVG duplications with imports
- Use shared CSS generation function
- Remove hardcoded constants

#### Step 4.4: Future extensibility
- Framework for adding balance/faucet icons later
- Consistent theming system
- Easy maintenance of all web assets

### 5. Benefits

- **DRY compliance**: Single source of truth for each SVG
- **Maintainability**: Update icon in one place
- **Consistency**: Same styling across all components  
- **Performance**: Constants are compile-time, no runtime overhead
- **Extensibility**: Easy to add more icons (balance 📊, faucet 🚰, etc.)
- **Type safety**: Rust compiler ensures all references are valid

### 6. Implementation Phases

1. **Phase 1**: Create web-assets crate with mining icon
2. **Phase 2**: Update pool to use shared crate
3. **Phase 3**: Update translator to use shared crate  
4. **Phase 4**: Verify no functionality changes
5. **Phase 5**: (Future) Add balance & faucet SVG icons

## Alternative Considered: Static Files

I also considered serving SVG files statically, but the current approach using data URIs in CSS is more efficient since:
- No additional HTTP requests
- Icons load instantly with CSS
- No file serving complexity
- Better for single-binary deployment

---

**Status**: Planning complete - ready for implementation. The changes will be purely refactoring with no functional changes to the web interface.