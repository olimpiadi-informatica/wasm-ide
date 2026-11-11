import {HighlightStyle, syntaxHighlighting} from "@codemirror/language";
import {Extension} from "@codemirror/state";
import {EditorView} from "@codemirror/view";
import {tags as t} from "@lezer/highlight";
import {solarizedDark, solarizedLight} from "@uiw/codemirror-theme-solarized";

interface EditorPalette {
  dark: boolean;
  background: string;
  foreground: string;
  gutterBackground: string;
  gutterForeground: string;
  gutterActiveForeground: string;
  gutterBorder: string;
  caret: string;
  selection: string;
  selectionMatch: string;
  lineHighlight: string;
  panelBackground: string;
  panelBorder: string;
  keyword: string;
  operator: string;
  variable: string;
  functionName: string;
  string: string;
  number: string;
  type: string;
  constant: string;
  comment: string;
  invalid: string;
}

function buildEditorTheme(p: EditorPalette): Extension {
  const uiTheme = EditorView.theme(
    {
      "&": {
        color: p.foreground,
        backgroundColor: p.background,
      },
      ".cm-content": {
        caretColor: p.caret,
      },
      ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: p.caret,
      },
      "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
        {
          backgroundColor: `${p.selection} !important`,
        },
      ".cm-selectionMatch": {
        backgroundColor: p.selectionMatch,
      },
      ".cm-activeLine": {
        backgroundColor: p.lineHighlight,
      },
      ".cm-gutters": {
        backgroundColor: p.gutterBackground,
        color: p.gutterForeground,
        borderRight: `1px solid ${p.gutterBorder}`,
      },
      ".cm-activeLineGutter": {
        backgroundColor: p.lineHighlight,
        color: p.gutterActiveForeground,
      },
      ".cm-panels": {
        backgroundColor: p.panelBackground,
        color: p.foreground,
      },
      ".cm-panels.cm-panels-top": {
        borderBottom: `1px solid ${p.panelBorder}`,
      },
      ".cm-panels.cm-panels-bottom": {
        borderTop: `1px solid ${p.panelBorder}`,
      },
      ".cm-tooltip": {
        backgroundColor: p.panelBackground,
        color: p.foreground,
        border: `1px solid ${p.panelBorder}`,
      },
      ".cm-tooltip .cm-tooltip-arrow:before": {
        borderTopColor: p.panelBorder,
        borderBottomColor: p.panelBorder,
      },
      ".cm-tooltip .cm-tooltip-arrow:after": {
        borderTopColor: p.panelBackground,
        borderBottomColor: p.panelBackground,
      },
      ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
        backgroundColor: p.selection,
        color: p.foreground,
      },
    },
    {dark: p.dark},
  );

  const highlight = HighlightStyle.define([
    {tag: [t.keyword, t.controlKeyword, t.moduleKeyword], color: p.keyword},
    {tag: [t.operator, t.operatorKeyword, t.punctuation], color: p.operator},
    {tag: [t.name, t.variableName, t.deleted, t.character, t.macroName], color: p.variable},
    {tag: [t.propertyName, t.definition(t.propertyName)], color: p.variable},
    {tag: [t.function(t.variableName), t.function(t.propertyName), t.labelName], color: p.functionName},
    {tag: [t.string, t.processingInstruction, t.inserted, t.special(t.string)], color: p.string},
    {tag: [t.number, t.changed, t.annotation, t.modifier, t.self, t.namespace], color: p.number},
    {tag: [t.typeName, t.className, t.tagName], color: p.type},
    {tag: [t.color, t.constant(t.name), t.standard(t.name), t.atom, t.bool, t.special(t.variableName)], color: p.constant},
    {tag: [t.url, t.escape, t.regexp, t.link], color: p.constant},
    {tag: [t.meta, t.comment], color: p.comment, fontStyle: "italic"},
    {tag: t.strong, fontWeight: "bold"},
    {tag: t.emphasis, fontStyle: "italic"},
    {tag: t.link, textDecoration: "underline"},
    {tag: t.heading, fontWeight: "bold", color: p.keyword},
    {tag: t.strikethrough, textDecoration: "line-through"},
    {tag: t.invalid, color: p.invalid},
  ]);

  return [uiTheme, syntaxHighlighting(highlight)];
}

const winterLight = buildEditorTheme({
  dark: false,
  background: "#ffffff",
  foreground: "#24292f",
  gutterBackground: "#f6f9fe",
  gutterForeground: "#6e7781",
  gutterActiveForeground: "#24292f",
  gutterBorder: "#e3e9f4",
  caret: "#0069ff",
  selection: "#cfe2ff",
  selectionMatch: "#e7f1ff",
  lineHighlight: "#3b82f612",
  panelBackground: "#f6f9fe",
  panelBorder: "#e3e9f4",
  keyword: "#cf222e",
  operator: "#0550ae",
  variable: "#24292f",
  functionName: "#8250df",
  string: "#0a3069",
  number: "#0550ae",
  type: "#953800",
  constant: "#0550ae",
  comment: "#6e7781",
  invalid: "#cf222e",
});

const catppuccinLatte = buildEditorTheme({
  dark: false,
  background: "#eff1f5",
  foreground: "#4c4f69",
  gutterBackground: "#e6e9ef",
  gutterForeground: "#8c8fa1",
  gutterActiveForeground: "#4c4f69",
  gutterBorder: "#dce0e8",
  caret: "#dc8a78",
  selection: "#ccd0da",
  selectionMatch: "#dce0e8",
  lineHighlight: "#4c4f6912",
  panelBackground: "#e6e9ef",
  panelBorder: "#dce0e8",
  keyword: "#8839ef",
  operator: "#04a5e5",
  variable: "#4c4f69",
  functionName: "#1e66f5",
  string: "#40a02b",
  number: "#fe640b",
  type: "#df8e1d",
  constant: "#fe640b",
  comment: "#8c8fa1",
  invalid: "#d20f39",
});

const nordLight = buildEditorTheme({
  dark: false,
  background: "#eceff4",
  foreground: "#2e3440",
  gutterBackground: "#e5e9f0",
  gutterForeground: "#7b88a1",
  gutterActiveForeground: "#2e3440",
  gutterBorder: "#d8dee9",
  caret: "#5e81ac",
  selection: "#cfd8e6",
  selectionMatch: "#dde4ef",
  lineHighlight: "#4c566a12",
  panelBackground: "#e5e9f0",
  panelBorder: "#d8dee9",
  keyword: "#5e81ac",
  operator: "#4c566a",
  variable: "#2e3440",
  functionName: "#3b6ea8",
  string: "#4f7a38",
  number: "#8f4e8b",
  type: "#2d7b82",
  constant: "#b35c2e",
  comment: "#7b88a1",
  invalid: "#bf616a",
});

const tokyoNight = buildEditorTheme({
  dark: true,
  background: "#0f172a",
  foreground: "#c0caf5",
  gutterBackground: "#0c1425",
  gutterForeground: "#565f89",
  gutterActiveForeground: "#c0caf5",
  gutterBorder: "#1e293b",
  caret: "#3abdf7",
  selection: "#1e3a5f",
  selectionMatch: "#172a46",
  lineHighlight: "#7aa2f714",
  panelBackground: "#0c1425",
  panelBorder: "#1e293b",
  keyword: "#bb9af7",
  operator: "#89ddff",
  variable: "#c0caf5",
  functionName: "#7aa2f7",
  string: "#9ece6a",
  number: "#ff9e64",
  type: "#2ac3de",
  constant: "#ff9e64",
  comment: "#565f89",
  invalid: "#f7768e",
});

const oneDark = buildEditorTheme({
  dark: true,
  background: "#282c34",
  foreground: "#abb2bf",
  gutterBackground: "#21252b",
  gutterForeground: "#636d83",
  gutterActiveForeground: "#abb2bf",
  gutterBorder: "#181a1f",
  caret: "#528bff",
  selection: "#3e4451",
  selectionMatch: "#323842",
  lineHighlight: "#abb2bf12",
  panelBackground: "#21252b",
  panelBorder: "#181a1f",
  keyword: "#c678dd",
  operator: "#56b6c2",
  variable: "#e06c75",
  functionName: "#61afef",
  string: "#98c379",
  number: "#d19a66",
  type: "#e5c07b",
  constant: "#d19a66",
  comment: "#5c6370",
  invalid: "#e06c75",
});

const dracula = buildEditorTheme({
  dark: true,
  background: "#282a36",
  foreground: "#f8f8f2",
  gutterBackground: "#232530",
  gutterForeground: "#6272a4",
  gutterActiveForeground: "#f8f8f2",
  gutterBorder: "#191a21",
  caret: "#f8f8f0",
  selection: "#44475a",
  selectionMatch: "#383a4c",
  lineHighlight: "#f8f8f212",
  panelBackground: "#232530",
  panelBorder: "#191a21",
  keyword: "#ff79c6",
  operator: "#ff79c6",
  variable: "#f8f8f2",
  functionName: "#50fa7b",
  string: "#f1fa8c",
  number: "#bd93f9",
  type: "#8be9fd",
  constant: "#bd93f9",
  comment: "#6272a4",
  invalid: "#ff5555",
});

const catppuccinMocha = buildEditorTheme({
  dark: true,
  background: "#1e1e2e",
  foreground: "#cdd6f4",
  gutterBackground: "#181825",
  gutterForeground: "#6c7086",
  gutterActiveForeground: "#cdd6f4",
  gutterBorder: "#11111b",
  caret: "#f5e0dc",
  selection: "#45475a",
  selectionMatch: "#313244",
  lineHighlight: "#cdd6f412",
  panelBackground: "#181825",
  panelBorder: "#313244",
  keyword: "#cba6f7",
  operator: "#89dceb",
  variable: "#cdd6f4",
  functionName: "#89b4fa",
  string: "#a6e3a1",
  number: "#fab387",
  type: "#f9e2af",
  constant: "#fab387",
  comment: "#6c7086",
  invalid: "#f38ba8",
});

const nordDark = buildEditorTheme({
  dark: true,
  background: "#2e3440",
  foreground: "#d8dee9",
  gutterBackground: "#272c36",
  gutterForeground: "#616e88",
  gutterActiveForeground: "#eceff4",
  gutterBorder: "#21262e",
  caret: "#d8dee9",
  selection: "#434c5e",
  selectionMatch: "#3b4252",
  lineHighlight: "#d8dee914",
  panelBackground: "#272c36",
  panelBorder: "#3b4252",
  keyword: "#81a1c1",
  operator: "#81a1c1",
  variable: "#d8dee9",
  functionName: "#88c0d0",
  string: "#a3be8c",
  number: "#b48ead",
  type: "#8fbcbb",
  constant: "#d08770",
  comment: "#616e88",
  invalid: "#bf616a",
});

const gruvboxDark = buildEditorTheme({
  dark: true,
  background: "#282828",
  foreground: "#ebdbb2",
  gutterBackground: "#1d2021",
  gutterForeground: "#7c6f64",
  gutterActiveForeground: "#fabd2f",
  gutterBorder: "#3c3836",
  caret: "#ebdbb2",
  selection: "#504945",
  selectionMatch: "#3c3836",
  lineHighlight: "#32302f",
  panelBackground: "#1d2021",
  panelBorder: "#3c3836",
  keyword: "#fb4934",
  operator: "#fe8019",
  variable: "#ebdbb2",
  functionName: "#b8bb26",
  string: "#b8bb26",
  number: "#d3869b",
  type: "#fabd2f",
  constant: "#d3869b",
  comment: "#928374",
  invalid: "#fb4934",
});

const editorThemes: Record<string, Extension> = {
  light: winterLight,
  catppuccin_latte: catppuccinLatte,
  solarized_light: solarizedLight,
  nord_light: nordLight,
  dark: tokyoNight,
  one_dark: oneDark,
  dracula,
  catppuccin_mocha: catppuccinMocha,
  nord: nordDark,
  gruvbox_dark: gruvboxDark,
  solarized_dark: solarizedDark,
};

export function resolveEditorTheme(name: string): Extension {
  return editorThemes[name] || winterLight;
}
