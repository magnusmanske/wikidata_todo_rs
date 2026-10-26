/**
 * Relator2-specific navbar.
 *
 * Mirrors the shared resources/vue_es6/tool-navbar.js, but:
 *   - shows the tool name as an always-visible navbar-brand (visible in both
 *     desktop and collapsed/mobile view, instead of being hidden inside the
 *     collapsible area),
 *   - hard-codes the name as "Relator2" rather than the shared tt='toolname'
 *     key (which resolves to "Relator" for this tool's translation set),
 *   - places the burger toggler on the right in mobile view,
 *   - lays its content out as two rows: language switcher + help + Toolforge
 *     logo on the first, the `actions` slot (login notice + random) on the
 *     second. On desktop both rows sit on a single non-wrapping line; in the
 *     opened mobile menu they stack.
 *
 * Slots:
 *   help    – first row, after the language switcher (the help button)
 *   actions – second row (login notice + random button)
 */

import { state } from '../../resources/vue_es6/state.js';

export default {
    name: 'tool-navbar',
    mounted() {
        if (state.tt) {
            state.tt.updateInterface(this.$el);
            state.tt.addILdropdown('#tooltranslate_wrapper');
        }
    },
    template: `
<nav class="navbar navbar-expand-lg bg-light" data-bs-theme="light">
    <div class="container-fluid">
        <a class="navbar-brand" href="#/">Relator2</a>
        <button class="navbar-toggler ms-auto" type="button" data-bs-toggle="collapse"
            data-bs-target="#navbarSupportedContent" aria-controls="navbarSupportedContent"
            aria-expanded="false" aria-label="Toggle navigation">
            <span class="navbar-toggler-icon"></span>
        </button>
        <div class="collapse navbar-collapse" id="navbarSupportedContent">
            <div class="relator2-nav-rows">
                <div class="relator2-nav-row">
                    <div id='tooltranslate_wrapper'></div>
                    <slot name='help'></slot>
                    <a style="display:inline-flex;align-items:center;" href="/" target='_blank'>
                        <img src="https://upload.wikimedia.org/wikipedia/commons/thumb/c/c5/Toolforge_logo.svg/60px-Toolforge_logo.svg.png"
                            tt_title="wmf_powered" border="0" width="32px">
                    </a>
                </div>
                <div class="relator2-nav-row">
                    <slot name='actions'></slot>
                </div>
            </div>
        </div>
    </div>
</nav>`,
};
