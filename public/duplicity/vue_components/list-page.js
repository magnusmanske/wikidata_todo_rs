import { ttMixin } from '../../resources/vue_es6/state.js';
import Pagination from '../../resources/vue_es6/pagination.js';
import { bus } from '../app-state.js';
import { apiJson } from '../api-client.js';
import { desktopServerForWiki } from '../wiki-utils.js';

// Rows rendered per page. The full list can hold >100k entries (issue #32),
// which the browser cannot render at once, so it is paged client-side.
const PER_PAGE = 200;

// UTF-8 byte-order mark; prepended to downloads so spreadsheet apps detect
// the encoding instead of mis-reading non-ASCII titles.
const BOM = '\ufeff';

// Columns written to a downloaded CSV/TSV file (issues #77, #80). check_url is
// the absolute link to this tool's per-article check page.
const DOWNLOAD_COLUMNS = ['title', 'creation_date', 'url', 'check_url'];

// CSV field per RFC 4180: quote when the value contains a comma, quote or
// line break, doubling any embedded quotes.
function csvField(value) {
    const s = String(value ?? '');
    return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

// TSV field. Wiki page titles cannot contain tabs or line breaks, but collapse
// them defensively so a stray value can never shift the columns.
function tsvField(value) {
    return String(value ?? '').replace(/[\t\r\n]+/g, ' ');
}

// Build the file body. A UTF-8 BOM is prepended so spreadsheet apps detect the
// encoding; rows are CRLF-separated for the same reason.
function buildDelimited(rows, format) {
    const sep = format === 'tsv' ? '\t' : ',';
    const escape = format === 'tsv' ? tsvField : csvField;
    return BOM + rows.map((row) => row.map(escape).join(sep)).join('\r\n');
}

// Hand a generated file to the browser via a transient object URL.
function triggerDownload(filename, mime, text) {
    const blob = new Blob([text], { type: `${mime};charset=utf-8` });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
}

export default {
    name: 'list-page',
    mixins: [ttMixin],
    components: { pagination: Pagination },
    props: ['wiki', 'category'],
    data() {
        // paginated defaults to true: the full list can hold >100k rows (#32),
        // so "show all" is an explicit opt-in via the toggle.
        return { articles: [], loading: true, failed: false, offset: 0, paginated: true };
    },
    computed: {
        perPage() { return PER_PAGE; },
        server() { return desktopServerForWiki(this.wiki) ?? ''; },
        excludeCategory() { return this.$route.query.exclude ?? ''; },
        pagedArticles() {
            return this.paginated
                ? this.articles.slice(this.offset, this.offset + PER_PAGE)
                : this.articles;
        },
    },
    created() {
        this.loadArticles();
    },
    watch: {
        // The exclude category lives in the query string, so navigating to or
        // from it reuses this component without re-running created(); reload.
        excludeCategory() { this.loadArticles(); },
    },
    methods: {
        articleUrl(title) {
            return `https://${this.server}/wiki/${encodeURI(title.replace(/ /g, '_'))}`;
        },
        articleRoute(title) {
            return `/article/${this.wiki}/${encodeURIComponent(title.replace(/ /g, '_'))}`;
        },
        // Absolute URL to the per-article check page (issue #80). The router runs
        // in hash mode, so resolve() yields a "#/article/..." href that is then
        // made absolute against the current page.
        checkUrl(title) {
            const href = this.$router.resolve(this.articleRoute(title)).href;
            return new URL(href, window.location.href).href;
        },
        // Download the whole (already-filtered) list in one file (issue #77).
        // The full list is held in memory; only its rendering is paged, so no
        // extra request is needed and the user gets every article at once.
        downloadList(format) {
            if (this.articles.length === 0) return;
            const rows = [
                DOWNLOAD_COLUMNS,
                ...this.articles.map((a) => [a.title, a.creation_date, this.articleUrl(a.title), this.checkUrl(a.title)]),
            ];
            const mime = format === 'tsv' ? 'text/tab-separated-values' : 'text/csv';
            triggerDownload(this.downloadFilename(format), mime, buildDelimited(rows, format));
        },
        downloadFilename(ext) {
            const category = this.category ? `-${this.category}` : '';
            return `duplicity-${this.wiki}${category}.${ext}`;
        },
        togglePaginated() {
            this.paginated = !this.paginated;
            this.offset = 0;
        },
        async loadArticles() {
            this.loading = true;
            this.failed = false;
            this.offset = 0;
            try {
                const params = { action: 'articles', wiki: this.wiki };
                if (this.category !== undefined) params.category = this.category;
                if (this.excludeCategory !== '') params.exclude_category = this.excludeCategory;
                const j = await apiJson('./api.php', params);
                if (j.status !== 'OK') {
                    bus.$emit('error', j.status);
                    this.failed = true;
                    return;
                }
                this.articles = j.articles;
            } catch (err) {
                this.failed = true;
                bus.$emit('error', `Failed to load articles: ${err.message || err}`);
            } finally {
                this.loading = false;
            }
        },
    },
    template: `
<div>
    <p class="lead" tt="list_blurb" :tt1="wiki"></p>

    <p v-if="excludeCategory !== ''" class="note">
        <span tt="excluding_category" :tt1="excludeCategory.replace(/_/g,' ')"></span>
        <router-link :to="'/list/'+wiki" class="hint" tt="show_all"></router-link>
    </p>

    <div v-if="loading" class="loading">
        <span class="spinner" role="status" aria-label="Loading"></span>
    </div>

    <template v-else-if="articles.length > 0">
        <div class="toolbar">
            <router-link :to="'/article/'+wiki" class="button small secondary" tt="check_random_article"></router-link>
            <button type="button" class="small secondary" @click.prevent="togglePaginated">
                <span v-if="paginated" tt="show_all_rows"></span>
                <span v-else tt="show_paginated"></span>
            </button>
            <button type="button" class="small" @click.prevent="downloadList('csv')" tt="download_csv"></button>
            <button type="button" class="small" @click.prevent="downloadList('tsv')" tt="download_tsv"></button>
        </div>

        <pagination
            v-if="paginated"
            :offset="offset"
            :items-per-page="perPage"
            :total="articles.length"
            :show-first-last="true"
            @go-to-page="offset = $event"
        ></pagination>

        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th class="num">#</th>
                        <th tt="article"></th>
                        <th tt="check_article"></th>
                        <th tt="article_created"></th>
                    </tr>
                </thead>
                <tbody>
                    <tr v-for="(a, num) in pagedArticles" :key="'row_'+(offset+num)">
                        <td class="num">{{offset + num + 1}}</td>
                        <td>
                            <a :href="articleUrl(a.title)" target="_blank">{{a.title}}</a>
                        </td>
                        <td>
                            <router-link target="_blank" :to="articleRoute(a.title)" tt="check"></router-link>
                        </td>
                        <td>{{a.creation_date}}</td>
                    </tr>
                </tbody>
            </table>
        </div>

        <pagination
            v-if="paginated && articles.length > perPage"
            :offset="offset"
            :items-per-page="perPage"
            :total="articles.length"
            :show-first-last="true"
            @go-to-page="offset = $event"
        ></pagination>
    </template>

    <p v-else-if="!failed" class="muted">No candidates at the moment.</p>
</div>`,
};
