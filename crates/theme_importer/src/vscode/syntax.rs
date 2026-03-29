use indexmap::IndexMap;
use serde::Deserialize;
use strum::EnumIter;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum VsCodeTokenScope {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct VsCodeTokenColor {
    pub name: Option<String>,
    pub scope: Option<VsCodeTokenScope>,
    pub settings: VsCodeTokenColorSettings,
}

#[derive(Debug, Deserialize)]
pub struct VsCodeTokenColorSettings {
    pub foreground: Option<String>,
    pub background: Option<String>,
    #[serde(rename = "fontStyle")]
    pub font_style: Option<String>,
}

#[derive(Debug, PartialEq, Copy, Clone, EnumIter)]
pub enum QuorpSyntaxToken {
    Attribute,
    Boolean,
    Comment,
    CommentDoc,
    Constant,
    Constructor,
    Embedded,
    Emphasis,
    EmphasisStrong,
    Enum,
    Function,
    Hint,
    Keyword,
    Label,
    LinkText,
    LinkUri,
    Number,
    Operator,
    Predictive,
    Preproc,
    Primary,
    Property,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationListMarker,
    PunctuationSpecial,
    String,
    StringEscape,
    StringRegex,
    StringSpecial,
    StringSpecialSymbol,
    Tag,
    TextLiteral,
    Title,
    Type,
    Variable,
    VariableSpecial,
    Variant,
}

impl std::fmt::Display for QuorpSyntaxToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                QuorpSyntaxToken::Attribute => "attribute",
                QuorpSyntaxToken::Boolean => "boolean",
                QuorpSyntaxToken::Comment => "comment",
                QuorpSyntaxToken::CommentDoc => "comment.doc",
                QuorpSyntaxToken::Constant => "constant",
                QuorpSyntaxToken::Constructor => "constructor",
                QuorpSyntaxToken::Embedded => "embedded",
                QuorpSyntaxToken::Emphasis => "emphasis",
                QuorpSyntaxToken::EmphasisStrong => "emphasis.strong",
                QuorpSyntaxToken::Enum => "enum",
                QuorpSyntaxToken::Function => "function",
                QuorpSyntaxToken::Hint => "hint",
                QuorpSyntaxToken::Keyword => "keyword",
                QuorpSyntaxToken::Label => "label",
                QuorpSyntaxToken::LinkText => "link_text",
                QuorpSyntaxToken::LinkUri => "link_uri",
                QuorpSyntaxToken::Number => "number",
                QuorpSyntaxToken::Operator => "operator",
                QuorpSyntaxToken::Predictive => "predictive",
                QuorpSyntaxToken::Preproc => "preproc",
                QuorpSyntaxToken::Primary => "primary",
                QuorpSyntaxToken::Property => "property",
                QuorpSyntaxToken::Punctuation => "punctuation",
                QuorpSyntaxToken::PunctuationBracket => "punctuation.bracket",
                QuorpSyntaxToken::PunctuationDelimiter => "punctuation.delimiter",
                QuorpSyntaxToken::PunctuationListMarker => "punctuation.list_marker",
                QuorpSyntaxToken::PunctuationSpecial => "punctuation.special",
                QuorpSyntaxToken::String => "string",
                QuorpSyntaxToken::StringEscape => "string.escape",
                QuorpSyntaxToken::StringRegex => "string.regex",
                QuorpSyntaxToken::StringSpecial => "string.special",
                QuorpSyntaxToken::StringSpecialSymbol => "string.special.symbol",
                QuorpSyntaxToken::Tag => "tag",
                QuorpSyntaxToken::TextLiteral => "text.literal",
                QuorpSyntaxToken::Title => "title",
                QuorpSyntaxToken::Type => "type",
                QuorpSyntaxToken::Variable => "variable",
                QuorpSyntaxToken::VariableSpecial => "variable.special",
                QuorpSyntaxToken::Variant => "variant",
            }
        )
    }
}

impl QuorpSyntaxToken {
    pub fn find_best_token_color_match<'a>(
        &self,
        token_colors: &'a [VsCodeTokenColor],
    ) -> Option<&'a VsCodeTokenColor> {
        let mut ranked_matches = IndexMap::new();

        for (ix, token_color) in token_colors.iter().enumerate() {
            if token_color.settings.foreground.is_none() {
                continue;
            }

            let Some(rank) = self.rank_match(token_color) else {
                continue;
            };

            if rank > 0 {
                ranked_matches.insert(ix, rank);
            }
        }

        ranked_matches
            .into_iter()
            .max_by_key(|(_, rank)| *rank)
            .map(|(ix, _)| &token_colors[ix])
    }

    fn rank_match(&self, token_color: &VsCodeTokenColor) -> Option<u32> {
        let candidate_scopes = match token_color.scope.as_ref()? {
            VsCodeTokenScope::One(scope) => vec![scope],
            VsCodeTokenScope::Many(scopes) => scopes.iter().collect(),
        }
        .iter()
        .flat_map(|scope| scope.split(',').map(|s| s.trim()))
        .collect::<Vec<_>>();

        let scopes_to_match = self.to_vscode();
        let number_of_scopes_to_match = scopes_to_match.len();

        let mut matches = 0;

        for (ix, scope) in scopes_to_match.into_iter().enumerate() {
            // Assign each entry a weight that is inversely proportional to its
            // position in the list.
            //
            // Entries towards the front are weighted higher than those towards the end.
            let weight = (number_of_scopes_to_match - ix) as u32;

            if candidate_scopes.contains(&scope) {
                matches += 1 + weight;
            }
        }

        Some(matches)
    }

    pub fn fallbacks(&self) -> &[Self] {
        match self {
            QuorpSyntaxToken::CommentDoc => &[QuorpSyntaxToken::Comment],
            QuorpSyntaxToken::Number => &[QuorpSyntaxToken::Constant],
            QuorpSyntaxToken::VariableSpecial => &[QuorpSyntaxToken::Variable],
            QuorpSyntaxToken::PunctuationBracket
            | QuorpSyntaxToken::PunctuationDelimiter
            | QuorpSyntaxToken::PunctuationListMarker
            | QuorpSyntaxToken::PunctuationSpecial => &[QuorpSyntaxToken::Punctuation],
            QuorpSyntaxToken::StringEscape
            | QuorpSyntaxToken::StringRegex
            | QuorpSyntaxToken::StringSpecial
            | QuorpSyntaxToken::StringSpecialSymbol => &[QuorpSyntaxToken::String],
            _ => &[],
        }
    }

    fn to_vscode(self) -> Vec<&'static str> {
        match self {
            QuorpSyntaxToken::Attribute => vec!["entity.other.attribute-name"],
            QuorpSyntaxToken::Boolean => vec!["constant.language"],
            QuorpSyntaxToken::Comment => vec!["comment"],
            QuorpSyntaxToken::CommentDoc => vec!["comment.block.documentation"],
            QuorpSyntaxToken::Constant => vec!["constant", "constant.language", "constant.character"],
            QuorpSyntaxToken::Constructor => {
                vec![
                    "entity.name.tag",
                    "entity.name.function.definition.special.constructor",
                ]
            }
            QuorpSyntaxToken::Embedded => vec!["meta.embedded"],
            QuorpSyntaxToken::Emphasis => vec!["markup.italic"],
            QuorpSyntaxToken::EmphasisStrong => vec![
                "markup.bold",
                "markup.italic markup.bold",
                "markup.bold markup.italic",
            ],
            QuorpSyntaxToken::Enum => vec!["support.type.enum"],
            QuorpSyntaxToken::Function => vec![
                "entity.function",
                "entity.name.function",
                "variable.function",
            ],
            QuorpSyntaxToken::Hint => vec![],
            QuorpSyntaxToken::Keyword => vec![
                "keyword",
                "keyword.other.fn.rust",
                "keyword.control",
                "keyword.control.fun",
                "keyword.control.class",
                "punctuation.accessor",
                "entity.name.tag",
            ],
            QuorpSyntaxToken::Label => vec![
                "label",
                "entity.name",
                "entity.name.import",
                "entity.name.package",
            ],
            QuorpSyntaxToken::LinkText => vec!["markup.underline.link", "string.other.link"],
            QuorpSyntaxToken::LinkUri => vec!["markup.underline.link", "string.other.link"],
            QuorpSyntaxToken::Number => vec!["constant.numeric", "number"],
            QuorpSyntaxToken::Operator => vec!["operator", "keyword.operator"],
            QuorpSyntaxToken::Predictive => vec![],
            QuorpSyntaxToken::Preproc => vec![
                "preproc",
                "meta.preprocessor",
                "punctuation.definition.preprocessor",
            ],
            QuorpSyntaxToken::Primary => vec![],
            QuorpSyntaxToken::Property => vec![
                "variable.member",
                "support.type.property-name",
                "variable.object.property",
                "variable.other.field",
            ],
            QuorpSyntaxToken::Punctuation => vec![
                "punctuation",
                "punctuation.section",
                "punctuation.accessor",
                "punctuation.separator",
                "punctuation.definition.tag",
            ],
            QuorpSyntaxToken::PunctuationBracket => vec![
                "punctuation.bracket",
                "punctuation.definition.tag.begin",
                "punctuation.definition.tag.end",
            ],
            QuorpSyntaxToken::PunctuationDelimiter => vec![
                "punctuation.delimiter",
                "punctuation.separator",
                "punctuation.terminator",
            ],
            QuorpSyntaxToken::PunctuationListMarker => {
                vec!["markup.list punctuation.definition.list.begin"]
            }
            QuorpSyntaxToken::PunctuationSpecial => vec!["punctuation.special"],
            QuorpSyntaxToken::String => vec!["string"],
            QuorpSyntaxToken::StringEscape => {
                vec!["string.escape", "constant.character", "constant.other"]
            }
            QuorpSyntaxToken::StringRegex => vec!["string.regex"],
            QuorpSyntaxToken::StringSpecial => vec!["string.special", "constant.other.symbol"],
            QuorpSyntaxToken::StringSpecialSymbol => {
                vec!["string.special.symbol", "constant.other.symbol"]
            }
            QuorpSyntaxToken::Tag => vec!["tag", "entity.name.tag", "meta.tag.sgml"],
            QuorpSyntaxToken::TextLiteral => vec!["text.literal", "string"],
            QuorpSyntaxToken::Title => vec!["title", "entity.name"],
            QuorpSyntaxToken::Type => vec![
                "entity.name.type",
                "entity.name.type.primitive",
                "entity.name.type.numeric",
                "keyword.type",
                "support.type",
                "support.type.primitive",
                "support.class",
            ],
            QuorpSyntaxToken::Variable => vec![
                "variable",
                "variable.language",
                "variable.member",
                "variable.parameter",
                "variable.parameter.function-call",
            ],
            QuorpSyntaxToken::VariableSpecial => vec![
                "variable.special",
                "variable.member",
                "variable.annotation",
                "variable.language",
            ],
            QuorpSyntaxToken::Variant => vec!["variant"],
        }
    }
}
