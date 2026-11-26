//! Connection colour and environment tag (`connTag.ts`): shown on the
//! explorer row, the editor, the console toolbar and the status bar, so it
//! is clear where a statement runs.

/// The palette: readable on both the light and the dark theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl ConnectionColor {
    pub const ALL: [ConnectionColor; 7] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Gray,
    ];

    /// The name stored in `connections.json` (`red`…).
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Gray => "gray",
        }
    }

    /// A palette colour by name; `None` for unknown names (hand-edited config).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The colour as `0xRRGGBB`.
    pub fn rgb(self) -> u32 {
        match self {
            Self::Red => 0xe5484d,
            Self::Orange => 0xf76b15,
            Self::Yellow => 0xd6a10b,
            Self::Green => 0x30a46c,
            Self::Blue => 0x0090ff,
            Self::Purple => 0x8e4ec6,
            Self::Gray => 0x8b8d98,
        }
    }

    /// The colour as CSS hex (`#e5484d`).
    pub fn hex(self) -> String {
        format!("#{:06x}", self.rgb())
    }
}

/// Environment tag of a connection (production always confirms destructive statements).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Environment {
    Development,
    Test,
    Staging,
    Production,
}

impl Environment {
    /// In menu order.
    pub const ALL: [Environment; 4] = [
        Self::Development,
        Self::Test,
        Self::Staging,
        Self::Production,
    ];

    /// The name stored in `connections.json` (`production`…).
    pub fn name(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == name)
    }

    /// Menu label: "Development"…
    pub fn label(self) -> &'static str {
        match self {
            Self::Development => "Development",
            Self::Test => "Test",
            Self::Staging => "Staging",
            Self::Production => "Production",
        }
    }

    /// Short badge text: "DEV", "TEST", "STAGE", "PROD".
    pub fn badge(self) -> &'static str {
        match self {
            Self::Development => "DEV",
            Self::Test => "TEST",
            Self::Staging => "STAGE",
            Self::Production => "PROD",
        }
    }

    /// Colour an environment gets when none is picked.
    pub fn color(self) -> ConnectionColor {
        match self {
            Self::Development => ConnectionColor::Green,
            Self::Test => ConnectionColor::Blue,
            Self::Staging => ConnectionColor::Orange,
            Self::Production => ConnectionColor::Red,
        }
    }
}

/// The connection's colour: the picked one (`color`, a palette name), else
/// its environment's; `None` when it has neither.
pub fn connection_color(
    environment: Option<Environment>,
    color: Option<&str>,
) -> Option<ConnectionColor> {
    color
        .and_then(ConnectionColor::from_name)
        .or_else(|| environment.map(Environment::color))
}

/// Tagged Production.
pub fn is_production(environment: Option<Environment>) -> bool {
    environment == Some(Environment::Production)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConnectionColor as C;
    use Environment as E;

    #[test]
    fn prefers_the_picked_colour_then_the_environment() {
        assert_eq!(connection_color(None, None), None);
        assert_eq!(connection_color(Some(E::Production), None), Some(C::Red));
        assert_eq!(
            connection_color(Some(E::Production), Some("purple")),
            Some(C::Purple)
        );
        assert_eq!(connection_color(None, Some("green")), Some(C::Green));
        assert_eq!(C::Red.hex(), "#e5484d");
        assert_eq!(C::Blue.hex(), "#0090ff");
    }

    #[test]
    fn ignores_unknown_colour_names() {
        assert_eq!(connection_color(None, Some("toString")), None);
        assert_eq!(
            connection_color(Some(E::Staging), Some("chartreuse")),
            Some(C::Orange)
        );
    }

    #[test]
    fn knows_production() {
        assert!(is_production(Some(E::Production)));
        assert!(!is_production(Some(E::Staging)));
        assert!(!is_production(None));
    }
}
