import {basicSetup} from "codemirror";
import {EditorView, keymap} from "@codemirror/view";
import {Compartment, Prec, EditorState, Facet} from "@codemirror/state";
import {cpp} from "@codemirror/lang-cpp";
import {python} from "@codemirror/lang-python";
import {emacs} from "@replit/codemirror-emacs";
import {vim} from "@replit/codemirror-vim";
import {indentWithTab} from "@codemirror/commands"
import {solarizedLight, solarizedDark} from "@uiw/codemirror-theme-solarized";
import {LSPClient, Transport, findReferencesKeymap, formatKeymap, jumpToDefinitionKeymap, languageServerExtensions, renameKeymap} from "@codemirror/lsp-client";

export class LSEventHandler implements Transport {
  subscribers = new Set<(value: string) => void>();

  constructor(public client: LSPClient, public sendMessage: (msg: string) => void) {}

  send(msg: string): void {
    this.sendMessage(msg);
  }

  subscribe(handler: (value: string) => void): void {
    this.subscribers.add(handler);
  }

  unsubscribe(handler: (value: string) => void): void {
    this.subscribers.delete(handler);
  }

  ready() {
    this.client.connect(this);
  }

  stopping() {
    this.client.disconnect();
  }

  message(msg: string) {
    this.subscribers.forEach((handler) => handler(msg));
  }
}

const useLast = (values: readonly any[]) => values.reduce((_, v) => v, "");
const languageId = Facet.define<string, string>({combine: useLast});

export class CM6Editor {
  language = new Compartment();
  keyboardMode = new Compartment();
  theme = new Compartment();
  isReadOnly = new Compartment();

  execCallback: () => void = () => {};
  onchangeCallback: () => void = () => {};
  execKeyBinding = Prec.highest(
    keymap.of([
      {
        key: "Mod-Enter",
        run: () => {
          this.execCallback();
          return true;
        },
      },
    ]),
  );
  view: EditorView;
  lsEventHandler: LSEventHandler;

  lspClient: LSPClient = new LSPClient({
    extensions: languageServerExtensions(),
  });
  lspPlugin = new Compartment();

  constructor(elementId: string) {
    const element = document.getElementById(elementId);
    if (element === null) {
      throw new Error(`"${elementId}" not found`);
    }
    this.view = new EditorView({
      extensions: [
        basicSetup,

        keymap.of([indentWithTab]),
        this.keyboardMode.of([]),
        this.execKeyBinding,
        this.theme.of(solarizedLight),
        this.language.of(languageId.of("")),
        this.isReadOnly.of(EditorState.readOnly.of(false)),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            this.onchangeCallback();
          }
        }),

        // LSP related plugins
        this.lspPlugin.of([]),
        keymap.of(formatKeymap),
        keymap.of(renameKeymap),
        keymap.of(jumpToDefinitionKeymap),
        keymap.of(findReferencesKeymap),
      ],
      parent: element,
    });
  }

  setLanguage(lang: string) {
    if (lang === "c") {
      this.view.dispatch({
        effects: this.language.reconfigure([cpp(), languageId.of("c")]),
      });
      this.view.dispatch({
        effects: this.lspPlugin.reconfigure([this.lspClient.plugin("file:///solution.c")]),
      });
    } else if (lang === "cpp") {
      this.view.dispatch({
        effects: this.language.reconfigure([cpp(), languageId.of("cpp")]),
      });
      this.view.dispatch({
        effects: this.lspPlugin.reconfigure([this.lspClient.plugin("file:///solution.cpp")]),
      });
    } else if (lang === "python") {
      this.view.dispatch({
        effects: this.language.reconfigure([python(), languageId.of("python")]),
      });
      this.view.dispatch({
        effects: this.lspPlugin.reconfigure([this.lspClient.plugin("file:///solution.py")]),
      });
    } else {
      this.view.dispatch({
        effects: this.language.reconfigure(languageId.of("")),
      });
    }
  }

  setDark(dark: boolean) {
    this.view.dispatch({
      effects: this.theme.reconfigure(dark ? solarizedDark : solarizedLight),
    });
  }

  setReadOnly(isReadonly: boolean) {
    this.view.dispatch({
      effects: this.isReadOnly.reconfigure(EditorState.readOnly.of(isReadonly)),
    });
  }

  setKeymap(keymap: string) {
    if (keymap === "vim") {
      this.view.dispatch({effects: this.keyboardMode.reconfigure(vim())});
    } else if (keymap === "emacs") {
      this.view.dispatch({effects: this.keyboardMode.reconfigure(emacs())});
    } else {
      this.view.dispatch({effects: this.keyboardMode.reconfigure([])});
    }
  }

  setExec(exec: () => void) {
    this.execCallback = exec;
  }

  setOnchange(onchange: () => void) {
    this.onchangeCallback = onchange;
  }

  setText(text: string) {
    this.view.dispatch({
      changes: {
        from: 0,
        to: this.view.state.doc.length,
        insert: text,
      },
    });
  }

  getText(): string {
    return this.view.state.doc.toString();
  }

  setLanguageServer(sendMessage: (msg: string) => void): LSEventHandler {
    this.lsEventHandler = new LSEventHandler(this.lspClient, sendMessage);
    return this.lsEventHandler;
  }
}
