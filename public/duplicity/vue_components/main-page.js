import { ttMixin } from '../../resources/vue_es6/state.js';
import { bus } from '../app-state.js';
import { apiJson } from '../api-client.js';

export default {
    name: 'main-page',
    mixins: [ttMixin],
    data() {
        return { wikis: [], loading: true };
    },
    created() {
        this.loadWikis();
    },
    methods: {
        async loadWikis() {
            this.loading = true;
            try {
                const j = await apiJson('./api.php', { action: 'wikis' });
                if (j.status !== 'OK') {
                    bus.$emit('error', j.status);
                    return;
                }
                this.wikis = j.wikis;
            } catch (err) {
                bus.$emit('error', `Failed to load wikis: ${err.message || err}`);
            } finally {
                this.loading = false;
            }
        },
    },
    template: `
<div class="container py-3">
    <p class="lead" tt="main_page_blurb"></p>
    <p tt="main_page_sub"></p>

    <div v-if="loading" class="text-center my-4">
        <div class="spinner-border" role="status" aria-hidden="true"></div>
    </div>

    <div v-else-if="wikis.length > 0">
        <h2 tt="available_wikis"></h2>
        <table class="table table-hover">
            <thead>
                <tr>
                    <th tt="wiki"></th>
                    <th tt="show_table_header"></th>
                    <th tt="random_article"></th>
                    <th class="text-end" tt="articles_without_items"></th>
                </tr>
            </thead>
            <tbody>
                <tr v-for="w in wikis" :key="w.wiki">
                    <th>{{w.wiki}}</th>
                    <td>
                        <router-link :to="'/list/'+w.wiki" tt="show_list"></router-link> |
                        <router-link :to="'/stats/'+w.wiki" tt="show_stats"></router-link> |
                        <router-link :to="'/categories/'+w.wiki" tt="show_categories"></router-link>
                    </td>
                    <td><router-link :to="'/article/'+w.wiki" tt="check_random_article"></router-link></td>
                    <td class="text-end font-monospace">{{w.cnt}}</td>
                </tr>
            </tbody>
        </table>
    </div>
</div>`,
};
