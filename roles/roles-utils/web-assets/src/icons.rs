use std::sync::OnceLock;

pub const PICKAXE_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M12.265 3.703c-2.536-.225-4.88.459-6.423 1.79-.19.164-.02.443.226.385 1.717-.41 3.67-.494 5.704-.197l.493-1.978zM15.168 6.527c1.935.693 3.62 1.685 4.944 2.853.189.166.472 0 .38-.235-.736-1.899-2.486-3.603-4.83-4.595l-.494 1.977zM12.481 5.936l1.94.484-1.209 4.851-1.94-.484zM10.787 10.667l2.91.726L11.4 20.61l-2.911-.726z\"/><path stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M12.358 3.329l3.396.847-.665 2.668-3.396-.847z\"/></svg>"##;
pub const WALLET_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path d=\"M15 17.5h3.005a1.5 1.5 0 001.5-1.5V8a1.5 1.5 0 00-1.5-1.5H15A1.5 1.5 0 0116.5 8v8a1.5 1.5 0 01-1.5 1.5z\"></path><rect width=\"12\" height=\"11\" x=\"4.5\" y=\"6.5\" rx=\"1.5\"></rect><circle cx=\"8.75\" cy=\"11.75\" r=\"1.25\"></circle></svg>"##;
pub const CLOCK_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><circle cx=\"12\" cy=\"12\" r=\"8.5\"></circle><path stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M12 7v5l2.8 2.8\"></path></svg>"##;
pub const FAUCET_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path stroke-linejoin=\"round\" d=\"M5.5 15H9v3.5H5.5zM15 5.5h3.5V9H15zM5.5 5.5H9V9H5.5zM11.75 5.5h.5V6h-.5zM11.75 8.625h.5v.5h-.5zM8.625 11.75h.5v.5h-.5zM11.75 14.875h.5v.5h-.5zM11.75 18h.5v.5h-.5zM5.5 11.75H6v.5h-.5zM11.75 11.75h.5v.5h-.5zM14.875 11.75h.5v.5h-.5zM18 11.75h.5v.5H18zM14.875 14.875h.5v.5h-.5zM18 14.875h.5 v.5H18zM14.875 18h.5v.5h-.5zM18 18h.5v.5H18z\"></path></svg>"##;
pub const MINER_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path d=\"M6.413 18.406a1.197 1.197 0 010-1.812A8.467 8.467 0 0112 14.5c2.139 0 4.093.79 5.587 2.094.553.483.553 1.329 0 1.812A8.467 8.467 0 0112 20.5a8.468 8.468 0 01-5.587-2.094zM8.521 8.5c.194 2.25 1.677 4 3.479 4s3.285-1.75 3.479-4H8.52z\"></path><path d=\"M16 8c0 .169-.008.336-.024.5H8.024A5.113 5.113 0 018 8c0-2.485 1.79-4.5 4-4.5s4 2.015 4 4.5zm-4-1a1 1 0 100-2 1 1 0 000 2z\" clip-rule=\"evenodd\"></path><path stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M7 8.5h10\"></path></svg>"##;
pub const BLOCK_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path d=\"M20.54 8.676v6.876a.694.694 0 01-.355.644l-7.132 4.024a2.096 2.096 0 01-2.056.002L3.82 16.197a.694.694 0 01-.355-.66V8.694a.694.694 0 01.345-.654l7.156-4.172a2.097 2.097 0 012.117.002l7.112 4.17a.693.693 0 01.344.636z\"></path><path d=\"M3.82 9.253a.699.699 0 01-.01-1.213l7.156-4.172a2.097 2.097 0 012.117.002l7.112 4.17a.699.699 0 01-.01 1.212l-7.132 4.024a2.096 2.096 0 01-2.056.003L3.82 9.253z\"></path></svg>"##;
pub const COINS_ICON_INLINE_SVG: &str = r##"<svg xmlns=\"http://www.w3.org/2000/svg\" fill=\"none\" viewBox=\"0 0 24 24\" stroke=\"#00ff00\"><path d=\"M16.495 10.255a6.5 6.5 0 01-6.24 6.24 4.5 4.5 0 106.24-6.24z\"></path><circle cx=\"10\" cy=\"10\" r=\"4.5\"></circle></svg>"##;

fn encode_for_data_uri(svg: &str) -> String {
    svg.replace('#', "%23")
}

static PICKAXE_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static PICKAXE_ICON_CSS: OnceLock<String> = OnceLock::new();
static WALLET_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static WALLET_ICON_CSS: OnceLock<String> = OnceLock::new();
static CLOCK_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static CLOCK_ICON_CSS: OnceLock<String> = OnceLock::new();
static FAUCET_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static FAUCET_ICON_CSS: OnceLock<String> = OnceLock::new();
static MINER_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static MINER_ICON_CSS: OnceLock<String> = OnceLock::new();
static BLOCK_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static BLOCK_ICON_CSS: OnceLock<String> = OnceLock::new();
static COINS_ICON_DATA_URI: OnceLock<String> = OnceLock::new();
static COINS_ICON_CSS: OnceLock<String> = OnceLock::new();
static NAV_ICON_CSS: OnceLock<String> = OnceLock::new();

pub fn pickaxe_icon_inline_svg() -> &'static str {
    PICKAXE_ICON_INLINE_SVG
}

pub fn pickaxe_icon_data_uri() -> &'static str {
    PICKAXE_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(PICKAXE_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn pickaxe_icon_css() -> &'static str {
    PICKAXE_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .pickaxe-icon::before {{
            content: '';
            display: inline-block;
            width: 1.2em;
            height: 1.2em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-image: url('{uri}');
            background-size: contain;
            background-repeat: no-repeat;
        }}
        a:hover .pickaxe-icon {{
            text-shadow: 0 0 10px #00ff00;
        }}
        a:hover .pickaxe-icon::before {{
            filter: drop-shadow(0 0 10px #00ff00);
        }}
        "#,
                uri = pickaxe_icon_data_uri()
            )
        })
        .as_str()
}

pub fn wallet_icon_inline_svg() -> &'static str {
    WALLET_ICON_INLINE_SVG
}

pub fn wallet_icon_data_uri() -> &'static str {
    WALLET_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(WALLET_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn wallet_icon_css() -> &'static str {
    WALLET_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .wallet-icon::before {{
            content: '';
            display: inline-block;
            width: 1.2em;
            height: 1.2em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-image: url('{uri}');
            background-size: contain;
            background-repeat: no-repeat;
        }}
        a:hover .wallet-icon {{
            text-shadow: 0 0 10px #00ff00;
        }}
        a:hover .wallet-icon::before {{
            filter: drop-shadow(0 0 10px #00ff00);
        }}
        "#,
                uri = wallet_icon_data_uri()
            )
        })
        .as_str()
}

pub fn clock_icon_inline_svg() -> &'static str {
    CLOCK_ICON_INLINE_SVG
}

pub fn clock_icon_data_uri() -> &'static str {
    CLOCK_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(CLOCK_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn clock_icon_css() -> &'static str {
    CLOCK_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .clock-icon {{
            display: inline-block;
            width: 1.2em;
            height: 1.2em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-color: currentColor;
            mask: url('{uri}') center / contain no-repeat;
            -webkit-mask: url('{uri}') center / contain no-repeat;
        }}
        "#,
                uri = clock_icon_data_uri()
            )
        })
        .as_str()
}

pub fn faucet_icon_inline_svg() -> &'static str {
    FAUCET_ICON_INLINE_SVG
}

pub fn faucet_icon_data_uri() -> &'static str {
    FAUCET_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(FAUCET_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn faucet_icon_css() -> &'static str {
    FAUCET_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .faucet-icon::before {{
            content: '';
            display: inline-block;
            width: 1.2em;
            height: 1.2em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-image: url('{uri}');
            background-size: contain;
            background-repeat: no-repeat;
        }}
        a:hover .faucet-icon {{
            text-shadow: 0 0 10px #00ff00;
        }}
        a:hover .faucet-icon::before {{
            filter: drop-shadow(0 0 12px #00ff00) drop-shadow(0 0 6px #00ff00);
        }}
        "#,
                uri = faucet_icon_data_uri()
            )
        })
        .as_str()
}

pub fn miner_icon_inline_svg() -> &'static str {
    MINER_ICON_INLINE_SVG
}

pub fn miner_icon_data_uri() -> &'static str {
    MINER_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(MINER_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn miner_icon_css() -> &'static str {
    MINER_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .miner-icon::before {{
            content: '';
            display: inline-block;
            width: 1.2em;
            height: 1.2em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-color: currentColor;
            mask: url('{uri}') center / contain no-repeat;
            -webkit-mask: url('{uri}') center / contain no-repeat;
        }}
        "#,
                uri = miner_icon_data_uri()
            )
        })
        .as_str()
}

pub fn block_icon_inline_svg() -> &'static str {
    BLOCK_ICON_INLINE_SVG
}

pub fn block_icon_data_uri() -> &'static str {
    BLOCK_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(BLOCK_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn block_icon_css() -> &'static str {
    BLOCK_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .block-icon::before {{
            content: '';
            display: inline-block;
            width: 1.1em;
            height: 1.1em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-image: url('{uri}');
            background-size: contain;
            background-repeat: no-repeat;
        }}
        "#,
                uri = block_icon_data_uri()
            )
        })
        .as_str()
}

pub fn coins_icon_inline_svg() -> &'static str {
    COINS_ICON_INLINE_SVG
}

pub fn coins_icon_data_uri() -> &'static str {
    COINS_ICON_DATA_URI
        .get_or_init(|| {
            format!(
                "data:image/svg+xml;charset=utf8,{}",
                encode_for_data_uri(COINS_ICON_INLINE_SVG)
            )
        })
        .as_str()
}

pub fn coins_icon_css() -> &'static str {
    COINS_ICON_CSS
        .get_or_init(|| {
            format!(
                r#"
        .coins-icon::before {{
            content: '';
            display: inline-block;
            width: 1.1em;
            height: 1.1em;
            vertical-align: middle;
            margin-right: 0.3em;
            background-image: url('{uri}');
            background-size: contain;
            background-repeat: no-repeat;
        }}
        "#,
                uri = coins_icon_data_uri()
            )
        })
        .as_str()
}

pub fn nav_icon_css() -> &'static str {
    NAV_ICON_CSS
        .get_or_init(|| {
            format!(
                "{}{}{}{}{}{}{}",
                wallet_icon_css(),
                pickaxe_icon_css(),
                clock_icon_css(),
                faucet_icon_css(),
                miner_icon_css(),
                block_icon_css(),
                coins_icon_css()
            )
        })
        .as_str()
}
