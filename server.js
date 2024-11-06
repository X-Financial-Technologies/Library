// server.js
const express = require('express');
const fs = require('fs');
const path = require('path');
const app = express();

app.use(express.static('.'));

const excludedItems = [
   'server.js',
   'script.js', 
   'index.html',
   'node_modules',
   '.git',
   '.gitignore',
   'package.json',
   'package-lock.json'
];

function getDirectoryContents(dirPath) {
   const items = fs.readdirSync(dirPath, { withFileTypes: true });
   return items
       .filter(item => !excludedItems.includes(item.name))
       .map(item => ({
           name: item.name,
           isDirectory: item.isDirectory(),
           path: path.join(dirPath, item.name).replace(/\\/g, '/')
       }));
}

app.get('/api/files/*', (req, res) => {
   const requestedPath = req.params[0] || '.';
   try {
       const contents = getDirectoryContents(requestedPath);
       res.json(contents);
   } catch (err) {
       res.status(500).json({ error: err.message });
   }
});

app.listen(3000);