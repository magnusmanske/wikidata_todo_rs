import { state, ttMixin } from '../../resources/vue_es6/state.js';
import { appState, bus, getWd } from '../app-state.js';
import {
    labelForProperty,
    mirror_properties,
    mirrorSelectionEvent,
    named_items,
    named_properties,
} from '../relations.js';
import { findDropRule, fallbackDropPropertyIds } from '../drop_rules.js';
import {
    hasMirrorProperty,
    isNonEmptyString,
    isSaveableDescription,
} from '../utils.js';
import { apiJson } from '../api-client.js';
import { isRejected, rejectSuggestion } from '../rejection_store.js';
import { relationWarnings, itemRelationWarnings } from '../sanity.js';

const HUMAN = 'Q5';
const INSTANCE_OF = 'P31';

// Common European language codes offered in the description language dropdown.
const DESCRIPTION_LANGUAGES = [
    'en', 'de', 'fr', 'es', 'it', 'nl', 'pt', 'pl', 'sv', 'da',
    'nb', 'fi', 'cs', 'ru', 'uk', 'el', 'hu', 'ro', 'ca',
];
const DESC_LANG_STORAGE_KEY = 'relator2_desc_lang';

function defaultDescLang() {
    return state.tt?.language || 'en';
}
// Remembered choice from a previous session, falling back to the UI language.
function loadDescLang() {
    try {
        return localStorage.getItem(DESC_LANG_STORAGE_KEY) || defaultDescLang();
    } catch (e) {
        return defaultDescLang();
    }
}
function storeDescLang(lang) {
    try {
        localStorage.setItem(DESC_LANG_STORAGE_KEY, lang);
    } catch (e) {
        /* private mode / storage disabled — choice simply isn't remembered */
    }
}

const WIKIPEDIA_URL = /^https?:\/\/([a-z-]+)\.wikipedia\.org\/wiki\/([^?#]+)/i;
const WIKIDATA_URL  = /^https?:\/\/(?:www\.)?wikidata\.org\/(?:wiki|entity)\/(Q\d+)\b/i;

function wikidataUrlToQid(url) {
    const m = url.match(WIKIDATA_URL);
    return m ? m[1].toUpperCase() : null;
}

// Extract a Qid from free-form text, e.g. "foo (Q42) bar" => "Q42".
// Returns the uppercased Qid, or null if none is found.
function extractQid(text) {
    const m = String(text ?? '').match(/\bQ\d+\b/i);
    return m ? m[0].toUpperCase() : null;
}

async function wikipediaUrlToQid(url) {
    const m = url.match(WIKIPEDIA_URL);
    if (!m) return null;
    const host = `${m[1].toLowerCase()}.wikipedia.org`;
    const title = decodeURIComponent(m[2]).replace(/_/g, ' ');
    const data = await apiJson(`https://${host}/w/api.php`, {
        action: 'query',
        titles: title,
        redirects: 1,
        prop: 'pageprops',
        ppprop: 'wikibase_item',
        format: 'json',
        origin: '*',
    });
    const pages = data.query?.pages ?? {};
    for (const page of Object.values(pages)) {
        const qid = page?.pageprops?.wikibase_item;
        if (qid && /^Q\d+$/.test(qid)) return qid;
    }
    return null;
}

export async function resolveDroppedUrlToQid(url) {
    return wikidataUrlToQid(url) ?? await wikipediaUrlToQid(url);
}

const definition = {
    name: 'person',
    mixins: [ttMixin],
    // hide_relations: suppress the live-relations table (the Diagram layout
    // already draws those relations as connecting lines). Defaults to false so
    // the Tree and Columns layouts are unchanged.
    props: {
        q: {},
        suggestions: {},
        is_busy: {},
        editing: {},
        hide_relations: { type: Boolean, default: false },
    },
    data() {
        return {
            adding_statement: false,
            adding_gender: false,
            editing_description: false,
            saving_description: false,
            description_draft: '',
            desc_lang: loadDescLang(),
            new_prop: '',
            new_q: '',
            adding_relation: false,
            drag_over: false,
            resolving_drop: false,
            drop_choice: null,
        };
    },
    created() {
        bus.$on('prefill-add-relation', this.onPrefillAddRelation);
        bus.$on('description-save-result', this.onDescriptionSaveResult);
    },
    beforeDestroy() {
        bus.$off('prefill-add-relation', this.onPrefillAddRelation);
        bus.$off('description-save-result', this.onDescriptionSaveResult);
    },
    watch: {
        desc_lang(lang) {
            storeDescLang(lang);
            // Switching language mid-edit should show that language's current
            // description, not the draft typed for the previous one.
            if (this.editing_description) this.description_draft = this.i?.getDesc(lang) ?? '';
        },
    },
    computed: {
        wd() { return getWd(); },
        main_q() { return appState.main_q; },
        hovered_q() { return appState.hovered_q; },
        is_main() { return this.q === appState.main_q; },
        named_items() { return named_items; },
        userLang() { return state.tt?.language || 'en'; },
        // Ensure the remembered/UI language is selectable even if it isn't one
        // of the common European codes listed above.
        descLangOptions() {
            return DESCRIPTION_LANGUAGES.includes(this.desc_lang)
                ? DESCRIPTION_LANGUAGES
                : [this.desc_lang, ...DESCRIPTION_LANGUAGES];
        },
        // appState.item_relations[q] is the reactive signal refreshed after every
        // edit to this item; wd.items is a plain (non-reactive) object, so the
        // header would otherwise show a stale snapshot. Touching it here keeps the
        // item reference (and the gender/dates derived from it) up to date.
        itemRelations() { return appState.item_relations[this.q]; },
        i() {
            void this.itemRelations;
            return getWd().getItem(this.q);
        },
        born() { return this.formatDateProperty(named_properties.birth_date); },
        died() { return this.formatDateProperty(named_properties.death_date); },
        is_male()   { return this.hasGender(named_items.male); },
        is_female() { return this.hasGender(named_items.female); },
        // Offer to add a gender only when the item has NO sex/gender (P21)
        // statement at all. An existing non-male/female value (e.g. non-binary)
        // still shows "?" but must not gain a contradictory male/female claim.
        has_gender_claim() { return !!this.i && this.i.hasClaims(named_properties.gender); },
        // Item-level warnings — the same person reached through more than one
        // relation (issue #75) — derived from this item's live relations only;
        // suggestions are vetted by the save gate before they are written.
        duplicateWarnings() {
            return itemRelationWarnings(appState.item_relations[this.q] ?? [], getWd());
        },
        relations() {
            const live  = appState.item_relations[this.q] ?? [];
            const suggs = (this.suggestions ?? []).filter((s) => !(s.suggestion && isRejected(s)));
            return [...live, ...suggs];
        },
        // With hide_relations (the Diagram layout) the live relations are drawn
        // as connecting lines, so only the actionable suggestion rows (the
        // selected/maybe checkboxes) are worth keeping in the expanded card.
        displayedRelations() {
            return this.hide_relations ? this.relations.filter((o) => o.suggestion) : this.relations;
        },
    },
    methods: {
        hasGender(gender_q) {
            return !!this.i && this.i.getClaimItemsForProperty(named_properties.gender, true).includes(gender_q);
        },
        // Prefer the selected description language so an edit made in it is
        // shown immediately; fall back to the mul/en/de… chain when it has none.
        description() { return this.i?.getDesc(this.desc_lang) || this.i?.getDesc() || ''; },
        labelFor(q) { return labelForProperty(q, getWd()); },
        // Logical-impossibility warnings (issue #75) for an existing relation
        // row, shown as a ⚠ icon. Suggestion/transient rows are skipped: the
        // save gate already vets those before they are written.
        relationIssues(o) {
            if (o.suggestion || o.transient) return [];
            const out = relationWarnings({ from: o.from, prop: o.prop, to: o.to }, getWd())
                .map((w) => w.message);
            for (const w of this.duplicateWarnings) {
                if (w.target === o.to) out.push(w.message);
            }
            return out;
        },

        startEditDescription() {
            if (this.editing || this.saving_description) return;
            this.description_draft = this.i?.getDesc(this.desc_lang) ?? '';
            this.editing_description = true;
            this.$nextTick(() => this.$refs.descInput?.focus());
        },
        cancelEditDescription() {
            this.editing_description = false;
            this.description_draft = '';
        },
        canSaveDescription() {
            return isSaveableDescription(this.description_draft, this.i?.getDesc(this.desc_lang) ?? '');
        },
        submitDescription() {
            if (this.editing || this.saving_description || !this.canSaveDescription()) return;
            this.saving_description = true;
            this.$emit('set-description', { q: this.q, lang: this.desc_lang, text: this.description_draft.trim() });
        },
        onDescriptionSaveResult({ q, ok }) {
            if (q !== this.q) return;
            this.saving_description = false;
            if (ok) {
                this.editing_description = false;
                this.description_draft = '';
            }
        },

        formatDateProperty(prop) {
            const claims = this.i?.bestRank(this.i.getClaimsForProperty(prop, true)) ?? [];
            if (claims.length === 0) return '';
            return this.formatDate(this.bestPrecision(claims));
        },
        formatDate(claim) {
            const value = this.i.getClaimDate(claim);
            if (!value) return '';
            let ret = value.time.replace(/^([+-]?[0-9-]+).*$/, '$1').replace(/^\+/, '');
            if (value.precision === 9)  ret = ret.replace(/-.*$/, '');
            if (value.precision === 10) ret = ret.replace(/-[^-]*$/, '');
            if (value.calendarmodel === 'http://www.wikidata.org/entity/Q11184') ret += 'J';
            return ret;
        },
        bestPrecision(claims) {
            return claims.reduce((best, c) => (c.precision > best.precision ? c : best), claims[0]);
        },

        targetLink(q) {
            let path = `./#/${q}`;
            if (isNonEmptyString(appState.source_url)) {
                path += `?ref=${encodeURIComponent(appState.source_url)}`;
            }
            return path;
        },

        onPrefillAddRelation({ q, prop, target_q }) {
            if (q !== this.q) return;
            if (!this.adding_statement) this.toggleAddingStatement();
            this.new_prop = prop;
            this.new_q = target_q;
        },

        toggleAddingGender() {
            this.adding_gender = !this.adding_gender;
        },
        chooseGender(gender_q) {
            if (this.editing || this.is_busy) return;
            this.adding_gender = false;
            this.$emit('set-gender', { q: this.q, gender_q });
        },

        toggleAddingStatement() {
            if (!this.adding_statement) {
                this.new_prop = '';
                this.new_q = '';
                document.querySelectorAll('a.adding_statement_active').forEach((a) => a.click());
            }
            this.adding_statement = !this.adding_statement;
        },
        canAddRelation() {
            return extractQid(this.new_q) !== null && hasMirrorProperty(this.new_prop, mirror_properties);
        },
        // Loading the items involved is a network round-trip, so the button
        // stays disabled via adding_relation until the suggestions are emitted;
        // without that, repeated clicks would each start another run.
        async addRelation() {
            if (this.adding_relation) return;
            this.adding_relation = true;
            try {
                await this.createRelation();
            } finally {
                this.adding_relation = false;
            }
        },
        async createRelation() {
            const new_q = extractQid(this.new_q);
            if (new_q === null) {
                bus.$emit('error', `Not a Qid: ${this.new_q}`);
                return;
            }
            const mirror_props = mirror_properties[this.new_prop];
            if (mirror_props === undefined) {
                bus.$emit('error', `Unsupported relation property: ${this.new_prop}`);
                return;
            }
            await getWd().getItemBatch([...mirror_props, new_q]);

            const me_q = this.q;
            const new_prop = this.new_prop;
            // The statement on the related item pointing back at this person is
            // always written. Relations such as stepparent (P3448) and godparent
            // (P1290) have no reciprocal property (empty mirror list), so that is
            // the only edit; others also get the gendered inverse on this item.
            const suggestions = [{ from: new_q, prop: new_prop, to: me_q, selected: true }];
            if (mirror_props.length > 0) {
                let prop = mirror_props[0];
                if (mirror_props.length > 1) {
                    const target = getWd().getItem(new_q);
                    if (target === undefined) {
                        bus.$emit('error', `Cannot determine the gender of ${new_q}`);
                        return;
                    }
                    if (target.getClaimItemsForProperty(named_properties.gender, true).includes(named_items.female)) {
                        prop = mirror_props[1];
                    }
                }
                suggestions.unshift({ from: me_q, prop, to: new_q, selected: true });
            }

            this.adding_statement = false;
            this.new_prop = '';
            this.new_q = '';
            this.$emit('add-suggestions', suggestions);
        },

        onSelectPerson() {
            const inputs = document.querySelectorAll('input.add_statement_new_q');
            if (inputs.length !== 1) return;
            const input = inputs[0];
            if (input.getAttribute('q') === this.q) return;
            input.value = this.q;
            input.dispatchEvent(new Event('input', { bubbles: true }));
        },
        onMouseEnter() { appState.hovered_q = this.q; },
        onMouseLeave() { if (appState.hovered_q === this.q) appState.hovered_q = ''; },

        onDragOver(event) {
            event.preventDefault();
            event.dataTransfer.dropEffect = 'link';
            this.drag_over = true;
        },
        onDragLeave() { this.drag_over = false; },
        async onDrop(event) {
            event.preventDefault();
            this.drag_over = false;
            const dt = event.dataTransfer;
            const url = (dt?.getData('text/uri-list') || dt?.getData('text/plain') || '').trim();
            await this.handleDroppedUrl(url);
        },
        // Resolve a dropped Wikidata/Wikipedia URL to a Qid and offer the right
        // relation. Split out from onDrop so the Diagram layout can forward a
        // drop landing on a collapsed box to the (just-expanded) person card.
        async handleDroppedUrl(url) {
            if (!url) return;
            this.resolving_drop = true;
            try {
                const q = await resolveDroppedUrlToQid(url);
                if (!q) {
                    bus.$emit('error', `No Wikidata item for ${url}`);
                    return;
                }
                await getWd().getItemBatch([q]);
                const target = getWd().getItem(q);
                const rule = findDropRule(target);
                if (rule) {
                    if (rule.properties.length === 1) {
                        this.emitDropSuggestion(rule.properties[0], q);
                    } else {
                        this.drop_choice = { rule, target_q: q };
                    }
                    return;
                }
                const instances = target?.getClaimItemsForProperty?.(INSTANCE_OF, true) ?? [];
                if (instances.includes(HUMAN)) {
                    if (!this.adding_statement) this.toggleAddingStatement();
                    this.new_q = q;
                    return;
                }
                this.drop_choice = {
                    rule: { name: 'fallback', properties: fallbackDropPropertyIds() },
                    target_q: q,
                };
            } catch (err) {
                bus.$emit('error', `Failed to resolve ${url}: ${err.message || err}`);
            } finally {
                this.resolving_drop = false;
            }
        },
        cancelDropChoice() { this.drop_choice = null; },
        emitDropSuggestion(prop, target_q) {
            this.$emit('add-suggestions', [{
                from: this.q,
                prop,
                to: target_q,
                transient: true,
            }]);
        },
        chooseDropProperty(prop) {
            if (!this.drop_choice) return;
            const target_q = this.drop_choice.target_q;
            this.drop_choice = null;
            this.emitDropSuggestion(prop, target_q);
        },

        rejectSuggestionRow(s) {
            if (!s.suggestion) return;
            if (s.selected) s.selected = false;
            rejectSuggestion(s);
        },
        onCheckboxChange(suggestion) {
            // v-model flips suggestion.selected on the same click; emit the
            // mirror edit so the reverse-direction suggestion stays in sync.
            const ev = mirrorSelectionEvent(suggestion, !(suggestion.selected ?? false));
            if (ev) this.$emit('select-suggestion', ev);
        },
    },
    template: `
<li class="person-card-wrapper" :key="'item_li_'+q">
    <div v-if="i===undefined" class="person-not-loaded">Could not load {{q}}</div>
    <div v-else
        :key="'item_div_'+q"
        :class="['card', 'person-card', { 'person-card-main': is_main, 'person-card-disabled': is_busy, 'person-card-drag-over': drag_over, 'person-card-resolving': resolving_drop }]"
        @click="onSelectPerson"
        @mouseenter="onMouseEnter" @mouseleave="onMouseLeave"
        @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop">
        <h5 class="card-header person-header">
            <a :href="targetLink(q)" tt_title="make_main_item">⌖</a>
            <wd-link :item="q" :wd="wd"></wd-link>
            <span class="person-gender">
                <span v-if="is_male">♂</span>
                <span v-else-if="is_female">♀</span>
                <span v-else-if="has_gender_claim">?</span>
                <span v-else-if="adding_gender" class="person-gender-picker" @click.stop>
                    <button type="button" class="person-gender-choice" tt_title="male"
                        :disabled="editing || is_busy" @click.stop.prevent="chooseGender(named_items.male)">♂</button>
                    <button type="button" class="person-gender-choice" tt_title="female"
                        :disabled="editing || is_busy" @click.stop.prevent="chooseGender(named_items.female)">♀</button>
                    <button type="button" class="person-gender-cancel" tt_title="cancel"
                        @click.stop.prevent="toggleAddingGender">×</button>
                </span>
                <span v-else class="person-gender-add" role="button" tabindex="0" tt_title="add_gender"
                    @click.stop="toggleAddingGender" @keyup.enter.stop="toggleAddingGender">?</span>
            </span>
            <small>[{{q}}]</small>
        </h5>
        <div class="card-body person-body" :key="'person_desc_'+q">
            <div class="person-description">
                <span v-if="editing_description" class="description-editor" @click.stop>
                    <select v-model="desc_lang" class="form-control form-control-sm description-lang"
                        :disabled="saving_description" tt_title="description_language">
                        <option v-for="lang in descLangOptions" :key="lang" :value="lang">{{lang}}</option>
                    </select>
                    <input ref="descInput" type="text" maxlength="250"
                        class="form-control form-control-sm description-input"
                        v-model="description_draft" :disabled="saving_description"
                        tt_placeholder="ph_description"
                        @keyup.enter="submitDescription" @keyup.esc="cancelEditDescription" />
                    <button type="button" class="description-save" tt_title="save_description"
                        :disabled="!canSaveDescription() || saving_description || editing"
                        @click.stop.prevent="submitDescription">✓</button>
                    <button type="button" class="description-cancel" tt_title="cancel"
                        :disabled="saving_description"
                        @click.stop.prevent="cancelEditDescription">×</button>
                </span>
                <span v-else class="description-display">
                    <span class="description-text">{{description()}}</span>
                    <button type="button"
                        :class="['description-edit-btn', { 'description-edit-empty': description() === '' }]"
                        :tt_title="description() === '' ? 'add_description' : 'edit_description'"
                        @click.stop.prevent="startEditDescription">✎</button>
                </span>
            </div>
            <div v-if="born+died !== ''" class="person-dates">
                {{born}}&nbsp;&mdash;&nbsp;{{died}}
            </div>
            <div v-if="drop_choice" class="drop-choice">
                <div class="drop-choice-prompt">
                    <span tt="this_is_the"></span>
                    <wd-link :item="drop_choice.target_q" :wd="wd" as_text="1"></wd-link>
                </div>
                <div class="drop-choice-buttons">
                    <button v-for="p in drop_choice.rule.properties" :key="p"
                        type="button" class="btn btn-sm btn-outline-primary"
                        @click.prevent="chooseDropProperty(p)">{{labelFor(p)}}</button>
                    <button type="button" class="btn btn-sm btn-outline-secondary"
                        @click.prevent="cancelDropChoice">&times;</button>
                </div>
            </div>
            <table v-if="!hide_relations || displayedRelations.length" class="person-relations">
                <tr v-for="(o,num) in displayedRelations" :key="o.from+'/'+o.prop+'/'+o.to+'/'+num"
                    :class="{ 'suggestion-row': o.suggestion }">
                    <td class="person-relations-icon">
                        <span v-if="o.transient && o.saved==='new'" class="relation-saved-check" aria-label="saved">✓</span>
                        <span v-else-if="o.transient && o.saved==='existed'" class="relation-saved-existed" aria-label="already exists" title="already exists">≡</span>
                        <span v-else-if="o.transient && o.saved==='error'" class="relation-saved-error" aria-label="save failed" :title="o.saved_msg || 'save failed'">✗</span>
                        <span v-else-if="o.transient" class="relation-saved-pending" aria-label="saving">…</span>
                        <input v-else-if="o.suggestion" type="checkbox" class="suggestion form-check-input"
                            v-model="o.selected" @click="onCheckboxChange(o)" />
                        <a v-else-if="o.to!=='somevalue' && o.to!=='novalue'" :href="targetLink(o.to)" tt_title="make_main_item">⌖</a>
                    </td>
                    <td><wd-link :item="o.prop" :wd="wd" as_text="1"></wd-link></td>
                    <td :class="{ 'related-highlight': o.to !== '' && o.to === hovered_q }">
                        <i v-if="o.to==='somevalue'" class="person-relations-special" tt="unknown_value"></i>
                        <i v-else-if="o.to==='novalue'" class="person-relations-special" tt="no_value"></i>
                        <b v-else-if="o.to===main_q"><wd-link :item="o.to" :wd="wd" as_text="1"></wd-link></b>
                        <wd-link v-else :item="o.to" :wd="wd" smallq="1"></wd-link>
                        <span v-if="o.maybe ?? false"><i tt="maybe"></i></span>
                        <span v-if="relationIssues(o).length" class="relation-warning"
                            role="img" aria-label="possible logical problem"
                            :title="relationIssues(o).join('\\n')">⚠️</span>
                        <button v-if="o.suggestion" type="button"
                            class="suggestion-reject"
                            tt_title="reject_suggestion"
                            title="don't suggest again"
                            @click.prevent="rejectSuggestionRow(o)">×</button>
                    </td>
                </tr>
            </table>
            <div class="d-flex">
                <span class="person-relations-icon">
                    <a href="#" @click.prevent="toggleAddingStatement"
                        tt_title="add_new_relation"
                        :class="['icon-link', 'person-add-toggle', { 'adding_statement_active': adding_statement }]">⊕</a>
                </span>
                <table v-if="adding_statement">
                    <tr>
                        <td tt="this_is_the"></td>
                        <td>
                            <select v-model="new_prop" class="form-select form-select-sm">
                                <option value="" disabled>&mdash;</option>
                                <option v-if="!is_female" value="P22">{{labelFor('P22')}}</option>
                                <option v-if="!is_male" value="P25">{{labelFor('P25')}}</option>
                                <option v-for="p in ['P3448','P26','P3373','P1038','P451','P40','P1066','P802','P3342']" :value="p" :key="p">{{labelFor(p)}}</option>
                            </select>
                        </td>
                    </tr>
                    <tr>
                        <td tt="of"></td>
                        <td>
                            <input type="text" placeholder="Qxxx" class="add_statement_new_q form-control form-control-sm d-inline-block w-auto"
                                v-model="new_q" :q="q" @keyup.enter="addRelation" />
                            <button type="button" class="btn btn-sm btn-outline-success" tt="add_relation"
                                :disabled="!canAddRelation() || editing || adding_relation" @click.prevent="addRelation"></button>
                        </td>
                    </tr>
                </table>
            </div>
        </div>
    </div>
</li>`,
};

Vue.component('person', definition);

export default definition;
