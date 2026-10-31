import { ttMixin } from '../../resources/vue_es6/state.js';
import { bus } from '../app-state.js';
import { apiJson } from '../api-client.js';
import { languageCodeForWiki } from '../wiki-utils.js';

export default {
    name: 'categories-page',
    mixins: [ttMixin],
    props: ['wiki'],
    data() {
        return { categories: [], loading: true };
    },
    created() {
        this.loadCategories();
    },
    methods: {
        petscanUrl(category) {
            let project = 'wikipedia';
            if (this.wiki === 'commonswiki') project = 'commons';
            else if (this.wiki === 'specieswiki') project = 'wikispecies';
            else if (this.wiki === 'wikidatawiki') project = 'wikidata';
            else {
                const m = this.wiki.match(/^.+(wik.+)$/);
                if (m && !/^wiki$/.test(m[1])) project = m[1];
            }
            const params = new URLSearchParams({
                language: languageCodeForWiki(this.wiki),
                project,
                categories: category,
                wikidata_item: 'without',
                doit: '1',
            });
            return `https://petscan.toolforge.org/?${params}`;
        },
        async loadCategories() {
            this.loading = true;
            try {
                const j = await apiJson('./api.php', { action: 'categories', wiki: this.wiki });
                if (j.status !== 'OK') {
                    bus.$emit('error', j.status);
                    return;
                }
                this.categories = j.categories;
            } catch (err) {
                bus.$emit('error', `Failed to load categories: ${err.message || err}`);
            } finally {
                this.loading = false;
            }
        },
    },
    template: `
<div class="container py-3">
    <p class="lead" tt="category_blurb" :tt1="wiki"></p>

    <div v-if="loading" class="text-center my-4">
        <div class="spinner-border" role="status" aria-hidden="true"></div>
    </div>

    <table v-else-if="categories.length > 0" class="table table-hover">
        <thead>
            <tr>
                <th tt="category"></th>
                <th class="text-end" tt="number_of_articles"></th>
                <th>PetScan</th>
            </tr>
        </thead>
        <tbody>
            <tr v-for="(c, idx) in categories" :key="'cat_'+idx">
                <td>
                    <router-link :to="'/list/'+wiki+'/'+encodeURIComponent(c[0])">
                        {{c[0].replace(/_/g,' ')}}
                    </router-link>
                    <router-link :to="{path: '/list/'+wiki, query: {exclude: c[0]}}"
                        class="small ms-2 text-muted" tt="exclude_from_list"></router-link>
                </td>
                <td class="text-end font-monospace">{{c[1]}}</td>
                <td class="text-end">
                    <a :href="petscanUrl(c[0])" target="_blank" rel="noopener">PetScan</a>
                </td>
            </tr>
        </tbody>
    </table>
</div>`,
};
