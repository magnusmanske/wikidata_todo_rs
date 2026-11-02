import { state, ttMixin } from '../../resources/vue_es6/state.js';
import { bus, getWd } from '../app-state.js';
import { apiJson } from '../api-client.js';
import { mobileServerForWiki, languageCodeForWiki } from '../wiki-utils.js';

const TAXON_RANK_QIDS = ['Q35409', 'Q34740', 'Q7432', 'Q68947'];
const HERBARIUM_QID = 'Q181916';
const TAXON_QID = 'Q16521';
const DISAMBIG_QIDS = ['Q4167410', 'Q22808320'];

const WIKIDATA_API  = 'https://www.wikidata.org/w/api.php';
const SPECIES_API   = 'https://species.wikimedia.org/w/api.php';
const LOCAL_API     = './api.php';
const INTERFACE_PAGE = 'Wikidata:Duplicity/P31';
const SET_SITELINK_URL = 'https://www.wikidata.org/wiki/Special:SetSiteLink';
const NEW_ITEM_URL = 'https://www.wikidata.org/wiki/Special:NewItem';

// Seconds to wait, with a cancel option, before committing a sitelink edit (issue #64).
const SITELINK_CONFIRM_SECONDS = 2;

// Search hits to offer as candidates.
const MAX_SEARCH_RESULTS = 100;

/**
 * Read-only Wikidata API call. Goes through our Toolforge proxy first; if that
 * fails (e.g. a 429 on the shared Toolforge egress), asks Wikidata directly from
 * the browser, which draws on the user's own rate-limit budget (issue #83).
 * The proxy is fetched without the shared retry helper so a rate limit there
 * fails over at once instead of first waiting out several Retry-After delays.
 */
async function wikidataApi(params) {
    const proxyUrl = `${LOCAL_API}?${new URLSearchParams({ action: 'wikidata_api_proxy', ...params })}`;
    const response = await fetch(proxyUrl).catch(() => null);
    if (response?.ok) return response.json();
    const { wdaction, ...rest } = params;
    return apiJson(WIKIDATA_API, { action: wdaction, format: 'json', ...rest });
}

function htmlText(html) {
    return new DOMParser().parseFromString(html, 'text/html').body.textContent ?? '';
}

export default {
    name: 'article-page',
    mixins: [ttMixin],
    props: ['wiki', 'fixed_page'],
    data() {
        return {
            already_used: false,
            page: '',
            current_q: '',
            search_query: '',
            search_results: [],
            no_search_results: false,
            is_working: false,
            work_cnt: 0,
            last_created_item_q: '',
            pending_sitelink: null,
            sitelink_seconds: 0,
            buttons: [
                { entries: [{ label: 'human', q: 'Q5' }] },
            ],
            species_group: TAXON_RANK_QIDS,
        };
    },
    computed: {
        wd() { return getWd(); },
        widar() { return state.widar; },
        tt() { return state.tt; },
        server() { return mobileServerForWiki(this.wiki) ?? ''; },
        loggedIn() { return !!state.widar?.is_logged_in; },
        userName() { return state.widar?.getUserName?.() ?? ''; },
        wikiUrl() { return this.page ? `https://${this.server}/wiki/${this.page}` : ''; },
        wikidataUrl() { return this.current_q ? `https://m.wikidata.org/wiki/${this.current_q}` : ''; },
        contributionsUrl() { return `https://www.wikidata.org/wiki/Special:Contributions/${encodeURIComponent(this.userName)}`; },
    },
    async created() {
        if (state.widar) state.widar.maximum_number_of_tries = 1;
        this._sitelink_timer = null;
        if (!this.server) {
            bus.$emit('error', `Cannot find server URL for wiki ${this.wiki}`);
        }
        await this.loadInterfaceButtons();
        await this.nextPage();
        this.update();
    },
    beforeDestroy() {
        // Abort any armed sitelink countdown so it cannot fire after the user
        // has navigated away from this page.
        this.cancelSitelink();
    },
    methods: {
        async loadInterfaceButtons() {
            try {
                const d = await wikidataApi({
                    wdaction: 'query',
                    prop: 'revisions',
                    rvprop: 'content',
                    rvslots: 'main',
                    formatversion: 2,
                    titles: INTERFACE_PAGE,
                });
                const lines = d.query.pages[0].revisions[0].slots.main.content.split('\n');
                const more = [];
                for (const line of lines) {
                    const m = line.match(/^;(.+?):([Q,0-9]+)\s*$/);
                    if (!m) continue;
                    more.push({
                        label: m[1],
                        entries: m[2].split(',').map((q) => ({ q })),
                    });
                }
                this.buttons = [...this.buttons, ...more];
            } catch (err) {
                bus.$emit('error', `Failed to load button list: ${err.message || err}`);
            }
        },

        computeSearchQuery() {
            const issn = this.page.match(/^ISSN[_ ](\S+)$/);
            if (issn) {
                this.search_query = `haswbstatement:"P236=${issn[1]}"`;
                return;
            }
            const initials = this.page.replace(/_/g, ' ').match(/^([A-Z]\. ?)+ ?\S+$/i);
            if (initials) {
                const stripped = this.page.replace(/_/g, ' ').replace(/\b([A-Z])\. */g, '$1* ');
                this.search_query = `"${stripped}" haswbstatement:P31=Q5`;
                return;
            }
            this.search_query = this.page
                .replace(/_/g, ' ')
                .replace(/ von /g, ' ')
                .replace(/-/g, ' ')
                .replace(/[A-Z]\./gi, ' ')
                .replace(/^[a-z ]+:/i, ' ')
                .replace(/\s*\(.+?\)\s*/g, ' ')
                .replace(/\s+/g, ' ')
                .trim();
        },

        async update() {
            this.computeSearchQuery();
            await this.search();
        },

        async search() {
            this.showQ('');
            this.search_results = [];
            this.no_search_results = false;
            if (this.search_query === '') return;

            this.working(true);
            try {
                const d = await wikidataApi({
                    wdaction: 'query',
                    list: 'search',
                    srnamespace: 0,
                    srlimit: MAX_SEARCH_RESULTS,
                    srsearch: this.search_query,
                });
                const ids = d.query.search.map((v) => v.title);
                if (ids.length === 0) {
                    this.no_search_results = true;
                    return;
                }
                await this.wd.getItemBatch(ids);
                this.search_results = ids;
                this.showQ(ids[0]);
            } catch (err) {
                bus.$emit('error', `Search failed: ${err.message || err}`);
            } finally {
                this.working(false);
            }
        },

        showQ(q) { this.current_q = q; },

        closeDropdown(event) {
            const dropdown = event.target.closest('details');
            if (dropdown) dropdown.open = false;
        },

        working(x) {
            this.work_cnt += x ? 1 : -1;
            this.is_working = this.work_cnt > 0;
        },

        liClass(q) {
            const parts = [];
            if (q === this.current_q) parts.push('selected');
            const i = this.wd.getItem(q);
            if (i !== undefined) {
                const sitelinks = i.getWikiLinks();
                if (sitelinks[this.wiki] !== undefined) parts.push('has_sitelink');
                const isDisambig = DISAMBIG_QIDS.some((q2) => i.hasClaimItemLink('P31', q2));
                if (isDisambig) parts.push('is_disambig');
            }
            return parts;
        },

        async runWidar(params) {
            if (!state.widar) throw new Error('widar not ready');
            return new Promise((resolve, reject) => {
                state.widar.run({ ...params, botmode: 1 }, (d) => {
                    if (!d || (d.error && d.error !== 'OK')) reject(new Error(d?.error || 'edit failed'));
                    else resolve(d);
                }, 1);
            });
        },
        setDescription(q, lang, desc)        { return this.runWidar({ action: 'set_desc',    q, lang, label: desc }); },
        setClaimItem(q, prop, target_q)      { return this.runWidar({ action: 'set_claims',  ids: q, prop, target: target_q }); },
        setClaimString(q, prop, text)        { return this.runWidar({ action: 'set_string',  id: q, prop, text }); },
        async createNewItem(wiki, page) {
            const d = await this.runWidar({ action: 'create_item_from_page', site: wiki, page });
            const q = d.res.entity.id;
            this.last_created_item_q = q;
            return q;
        },

        skip() {
            this.cancelSitelink();
            this.nextPage().then(() => this.update());
        },

        createNewItemFromPage() {
            this.cancelSitelink();
            const titleParam = encodeURIComponent(this.page.replace(/_/g, ' '));
            const lang = languageCodeForWiki(this.wiki);
            const labelGuess = encodeURIComponent(this.page.replace(/_/g, ' ').replace(/\(.+?\)$/, '').trim());
            const url = `${NEW_ITEM_URL}?lang=${lang}&site=${this.wiki}&page=${titleParam}&label=${labelGuess}`;
            window.open(url, '_blank');
            this.markPageDone(this.wiki, this.page);
            this.nextPage().then(() => this.update());
        },

        sitelinkExists(q) {
            const item = this.wd.getItem(q);
            if (item === undefined) return false;
            const links = item.getWikiLinks();
            return !!links && links[this.wiki] !== undefined;
        },

        requestAddToCurrentQ() {
            const wiki = this.wiki;
            const page = this.page;
            const q = this.current_q;
            if (q === '') return;
            if (!this.loggedIn) {
                // Anonymous users go to the on-wiki form, which has its own
                // confirmation, so no local delay is needed here.
                const url = `${SET_SITELINK_URL}?id=${q}&site=${wiki}&page=${encodeURIComponent(page.replace(/_/g, ' '))}`;
                window.open(url, '_blank');
                return;
            }
            // Arm a short, cancellable countdown before writing the sitelink so an
            // accidental click can be aborted before any edit is made (issue #64).
            // Capture the target now so it cannot drift if the search selection
            // changes while the countdown is running.
            this.cancelSitelink();
            this.pending_sitelink = { wiki, page, q, already_linked: this.sitelinkExists(q) };
            this.sitelink_seconds = SITELINK_CONFIRM_SECONDS;
            this._sitelink_timer = setInterval(() => {
                this.sitelink_seconds -= 1;
                if (this.sitelink_seconds <= 0) this.commitSitelink();
            }, 1000);
        },

        cancelSitelink() {
            if (this._sitelink_timer) {
                clearInterval(this._sitelink_timer);
                this._sitelink_timer = null;
            }
            this.pending_sitelink = null;
            this.sitelink_seconds = 0;
        },

        async commitSitelink() {
            if (this._sitelink_timer) {
                clearInterval(this._sitelink_timer);
                this._sitelink_timer = null;
            }
            const pending = this.pending_sitelink;
            this.pending_sitelink = null;
            this.sitelink_seconds = 0;
            if (!pending) return;
            const { wiki, page, q } = pending;
            this.nextPage().then(() => this.update());
            try {
                await this.runWidar({ action: 'set_sitelink', q, site: wiki, title: page });
                await this.markPageDone(wiki, page);
            } catch (err) {
                bus.$emit('error', `Sitelink failed: ${err.message || err}`);
            }
        },

        async createItemWithP31(p31_q) {
            this.cancelSitelink();
            if (!this.loggedIn) { bus.$emit('error', 'Not logged in'); return; }
            const wiki = this.wiki;
            const page = this.page;
            this.nextPage().then(() => this.update());
            this.working(true);
            try {
                const newQ = await this.createNewItem(wiki, page);
                await this.setClaimItem(newQ, 'P31', p31_q);
                await this.markPageDone(wiki, page);
            } catch (err) {
                bus.$emit('error', `Item creation failed: ${err.message || err}`);
            } finally {
                this.working(false);
            }
        },

        async createItemWithTaxon(taxon_rank_q) {
            this.cancelSitelink();
            if (!this.loggedIn) { bus.$emit('error', 'Not logged in'); return; }
            const wiki = this.wiki;
            const page = this.page;
            this.nextPage().then(() => this.update());
            this.working(true);
            try {
                const newQ = await this.createNewItem(wiki, page);
                await this.setClaimItem(newQ, 'P31', TAXON_QID);
                await this.setClaimItem(newQ, 'P105', taxon_rank_q);
                await this.setClaimString(newQ, 'P225', page.replace(/_/g, ' '));
                const parent = await this.detectParentTaxon(page);
                if (parent) await this.setClaimItem(newQ, 'P171', parent);
                await this.markPageDone(wiki, page);
            } catch (err) {
                bus.$emit('error', `Taxon creation failed: ${err.message || err}`);
            } finally {
                this.working(false);
            }
        },

        async createItemWithHerbarium() {
            this.cancelSitelink();
            if (!this.loggedIn) { bus.$emit('error', 'Not logged in'); return; }
            const wiki = this.wiki;
            const page = this.page;
            this.nextPage().then(() => this.update());
            this.working(true);
            try {
                const newQ = await this.createNewItem(wiki, page);
                await this.setClaimItem(newQ, 'P31', HERBARIUM_QID);
                await this.setClaimString(newQ, 'P5858', page.replace(/_/g, ' '));
                const desc = await this.detectHerbariumInfo(page);
                if (desc) await this.setDescription(newQ, 'en', desc);
                await this.markPageDone(wiki, page);
            } catch (err) {
                bus.$emit('error', `Herbarium creation failed: ${err.message || err}`);
            } finally {
                this.working(false);
            }
        },

        async detectHerbariumInfo(page) {
            try {
                const j = await apiJson(SPECIES_API, { action: 'parse', format: 'json', section: 0, prop: 'text', page });
                const text = htmlText(j.parse.text['*']);
                const m = text.match(/^Name: (.+?)\n/);
                return (m?.[1] ?? '').trim();
            } catch {
                return '';
            }
        },

        async detectParentTaxon(page) {
            let parent_name = '';
            let parent_taxon = '';
            const sub = page.match(/^(\S+)[ _](\S+)[ _]subsp\.[ _](\S+)$/) ?? page.match(/^(\S+)[ _](\S+)[ _](\S+)$/);
            if (sub) {
                parent_name = `${sub[1]} ${sub[2]}`;
                parent_taxon = 'Q7432';
            } else {
                const m = page.match(/^(\S+)[ _](\S+)$/);
                if (m) {
                    parent_name = m[1];
                    parent_taxon = 'Q34740';
                }
            }
            if (!parent_taxon) return '';
            try {
                const d = await wikidataApi({
                    wdaction: 'query',
                    list: 'search',
                    srsearch: `"${parent_name}" haswbstatement:P105=${parent_taxon}`,
                    srnamespace: 0,
                });
                if (d.query.search.length !== 1) return '';
                return d.query.search[0].title;
            } catch {
                return '';
            }
        },

        async markPageDone(wiki, page) {
            try {
                await apiJson(LOCAL_API, { action: 'invalidate_page', wiki, page });
            } catch {
                /* best-effort */
            }
            this.finishOperation();
        },

        finishOperation() {
            if (this.fixed_page === undefined) return;
            window.close();
        },

        async checkAlreadyUsed() {
            this.already_used = false;
            try {
                const d = await wikidataApi({ wdaction: 'wbgetentities', sites: this.wiki, titles: this.page });
                const keys = Object.keys(d.entities);
                this.already_used = keys[0] !== '-1';
                if (this.already_used) {
                    const wiki = this.wiki;
                    const page = this.page;
                    await apiJson(LOCAL_API, { action: 'invalidate_page', wiki, page });
                    if (this.fixed_page === undefined) {
                        await this.nextPage();
                        this.update();
                    }
                }
            } catch (err) {
                bus.$emit('error', `Already-used check failed: ${err.message || err}`);
            }
        },

        async nextPage() {
            this.already_used = false;
            this.current_q = '';
            this.search_query = '';
            this.search_results = [];
            this.no_search_results = false;
            if (this.fixed_page) {
                if (this.page === '') this.page = this.fixed_page;
                return;
            }
            try {
                const j = await apiJson(LOCAL_API, { action: 'random_nowd_page', wiki: this.wiki });
                if (j.status !== 'OK') {
                    bus.$emit('error', j.status);
                    throw new Error(j.status);
                }
                this.page = j.page;
                await this.checkAlreadyUsed();
            } catch (err) {
                bus.$emit('error', `Failed to fetch a random page: ${err.message || err}`);
                throw err;
            }
        },
    },
    template: `
<div class="article-page">
    <div class="toolbar article-toolbar">
        <button v-if="!fixed_page" class="small secondary" @click.prevent="skip" tt="skip"></button>
        <button class="small" @click.prevent="createNewItemFromPage" tt="button_new_item"></button>

        <template v-for="(g, gIdx) in buttons">
            <button v-if="g.entries.length === 1"
                :key="'btn_'+gIdx"
                class="small secondary"
                @click.prevent="createItemWithP31(g.entries[0].q)"
                :tt="g.entries[0].label ? 'button_'+g.entries[0].label : null">
                <wd-link v-if="!g.entries[0].label" :item="g.entries[0].q" as_text="1" :language="tt && tt.language"></wd-link>
            </button>
            <details v-else :key="'group_'+gIdx" class="dropdown">
                <summary class="button small secondary" :tt="'group_'+g.label"></summary>
                <div class="dropdown-menu">
                    <button v-for="(e, eIdx) in g.entries" :key="'opt_'+gIdx+'_'+eIdx"
                        @click.prevent="createItemWithP31(e.q); closeDropdown($event)"
                        :tt="e.label ? 'button_'+e.label : null">
                        <wd-link v-if="!e.label" :item="e.q" as_text="1" :language="tt && tt.language"></wd-link>
                    </button>
                </div>
            </details>
        </template>

        <details v-if="wiki === 'specieswiki'" class="dropdown">
            <summary class="button small secondary" tt="group_taxon"></summary>
            <div class="dropdown-menu">
                <button v-for="q in species_group" :key="'taxon_'+q"
                    @click.prevent="createItemWithTaxon(q); closeDropdown($event)">
                    <wd-link :item="q" as_text="1" :language="tt && tt.language"></wd-link>
                </button>
                <button @click.prevent="createItemWithHerbarium(); closeDropdown($event)">
                    <wd-link item="Q181916" as_text="1" :language="tt && tt.language"></wd-link>
                </button>
            </div>
        </details>

        <template v-if="pending_sitelink">
            <span v-if="pending_sitelink.already_linked" class="badge warn"
                tt="sitelink_will_change" :tt1="wiki"></span>
            <span class="hint" tt="sitelink_pending" :tt1="sitelink_seconds"></span>
            <button class="small danger" @click.prevent="cancelSitelink" tt="cancel"></button>
        </template>
        <button v-else-if="current_q" class="small"
            @click.prevent="requestAddToCurrentQ" tt="add" :tt1="current_q"></button>

        <span v-if="loggedIn" class="hint">
            [<a :href="contributionsUrl" target="_blank" tt="your_last_edits"></a>]
            <span v-if="last_created_item_q">
                [<wd-link :item="last_created_item_q"></wd-link>]
            </span>
        </span>

        <span v-if="is_working" class="spinner small" role="status" aria-label="Working"></span>
    </div>

    <div class="article-grid">
        <div>
            <p v-if="already_used" class="note" tt="already_linked"></p>
            <iframe v-if="page" :src="wikiUrl" :title="page" class="article-frame"></iframe>
        </div>

        <div class="article-search-col">
            <form class="article-search" @submit.prevent="search">
                <input type="text" v-model="search_query" aria-label="Search" />
                <button class="small secondary" type="submit">🔍</button>
            </form>
            <p v-if="no_search_results" class="hint"><i tt="no_search_results"></i></p>
            <ol v-else class="article-search-results">
                <li v-for="q in search_results" :class="liClass(q)" :key="'li_'+q">
                    <a href="#" @click.prevent="showQ(q)">
                        <small><wd-link :item="q" as_text="1"></wd-link></small>
                    </a>
                    <a :href="'https://www.wikidata.org/wiki/'+q" target="_blank">🔗</a>
                </li>
            </ol>
        </div>

        <div>
            <iframe v-if="current_q" :src="wikidataUrl" :title="current_q" class="article-frame"></iframe>
        </div>
    </div>
</div>`,
};
