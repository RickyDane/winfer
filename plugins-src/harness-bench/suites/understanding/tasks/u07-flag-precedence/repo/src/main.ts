import { flags } from './flags';

document.body.dataset.theme = flags.theme;
if (flags.betaBanner) document.body.classList.add('beta');
console.log('storefront', flags);
