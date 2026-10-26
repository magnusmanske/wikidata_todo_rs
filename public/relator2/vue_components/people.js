import { ttMixin } from '../../resources/vue_es6/state.js';
import './person.js';

const definition = {
    name: 'people',
    mixins: [ttMixin],
    props: ['items', 'suggestions', 'is_busy', 'editing'],
    methods: {
        suggestionsForQ(q) { return (this.suggestions ?? []).filter((s) => s.from === q); },
        forwardAddSuggestions(ev) { this.$emit('add-suggestions', ev); },
        forwardChangeMainQ(ev)    { this.$emit('change-main-q', ev); },
        forwardSelectSuggestion(ev) { this.$emit('select-suggestion', ev); },
        forwardSetGender(ev)        { this.$emit('set-gender', ev); },
        forwardSetDescription(ev)   { this.$emit('set-description', ev); },
    },
    template: `
<div class="people-group">
    <ul class="people-list">
        <person v-for="q in items" :key="'person_'+q"
            :q="q"
            :is_busy="(is_busy[q] ?? 0) > 0"
            :editing="editing"
            :suggestions="suggestionsForQ(q)"
            @add-suggestions="forwardAddSuggestions"
            @change-main-q="forwardChangeMainQ"
            @select-suggestion="forwardSelectSuggestion"
            @set-gender="forwardSetGender"
            @set-description="forwardSetDescription"></person>
    </ul>
</div>`,
};

Vue.component('people', definition);

export default definition;
