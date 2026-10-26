import { ttMixin } from '../../resources/vue_es6/state.js';
import { bus } from '../app-state.js';
import { apiJson } from '../api-client.js';

const MAX_X_TICKS = 15;

function packedDateToCoord(packed) {
    const year  = Number(packed.substr(0, 4));
    const month = Number(packed.substr(4, 2));
    const day   = Number(packed.substr(6, 2));
    return new Date(year, month, day).getTime() / 10000;
}

function packedDateLabel(packed) {
    return `${packed.substr(0, 4)}-${packed.substr(4, 2)}`;
}

export default {
    name: 'stats-page',
    mixins: [ttMixin],
    props: ['wiki'],
    data() {
        return { data: {}, dates: [], loading: true, logScale: false };
    },
    created() {
        this.loadStats();
    },
    watch: {
        logScale() { this.$nextTick(() => this.plot()); },
    },
    methods: {
        async loadStats() {
            this.loading = true;
            try {
                const j = await apiJson('./api.php', { action: 'stats', wiki: this.wiki });
                if (j.status !== 'OK') {
                    bus.$emit('error', j.status);
                    return;
                }
                this.data = j.data;
                this.dates = Object.keys(j.data).sort();
                this.$nextTick(() => this.plot());
            } catch (err) {
                bus.$emit('error', `Failed to load stats: ${err.message || err}`);
            } finally {
                this.loading = false;
            }
        },
        plot() {
            if (typeof $?.plot !== 'function') {
                bus.$emit('error', 'flot is not available');
                return;
            }
            const series = [this.dates.map((d) => [packedDateToCoord(d), this.data[d]])];

            let stride = 1;
            while (this.dates.length / stride > MAX_X_TICKS) stride++;
            const xticks = this.dates
                .map((date, idx) => (((idx + 1) % stride === 0) ? [packedDateToCoord(date), packedDateLabel(date)] : null))
                .filter(Boolean);

            const yaxis = this.logScale
                ? { transform: (v) => Math.log(v + 1), inverseTransform: (v) => Math.exp(v) - 1, min: 0 }
                : { min: 0 };

            $.plot('#duplicity_stats_graph', series, {
                series: { lines: { show: true }, points: { show: true } },
                xaxis: { ticks: xticks },
                yaxis,
                grid: { backgroundColor: { colors: ['#fff', '#eee'] } },
            });
        },
    },
    template: `
<div class="container py-3">
    <p class="lead" tt="stats_blurb" :tt1="wiki"></p>

    <div v-if="loading" class="text-center my-4">
        <div class="spinner-border" role="status" aria-hidden="true"></div>
    </div>

    <div v-show="!loading" class="form-check mb-2">
        <input class="form-check-input" type="checkbox" id="log-scale-toggle" v-model="logScale">
        <label class="form-check-label" for="log-scale-toggle">Logarithmic Y axis</label>
    </div>
    <div v-show="!loading" id="duplicity_stats_graph" class="duplicity-stats-graph"></div>
</div>`,
};
